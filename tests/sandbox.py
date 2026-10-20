#!/usr/bin/env python3
"""Verify the shipped systemd sandbox with isolated transient units.
Called only by e2e.py when NEEDLE_SYSTEMD=1. Creates and removes task-owned
/run and /var/lib directories; never changes installed service units.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import uuid


def verify(root, work, binary, cfg, viewer, request, check):
    suffix = uuid.uuid4().hex[:10]
    panel_unit = f'needle-check-panel-{suffix}'
    agent_unit = f'needle-check-agent-{suffix}'
    panel_state = Path('/var/lib') / panel_unit
    agent_state = Path('/var/lib') / agent_unit
    runtime = Path('/run') / panel_unit
    targets = []
    sandbox_binary = Path('/opt') / f'{panel_unit}-binary'
    shutil.copyfile(binary, sandbox_binary)
    os.chmod(sandbox_binary, 0o755)

    def call(*args, ok=True):
        result = subprocess.run([str(x) for x in args], capture_output=True, text=True)
        if ok and result.returncode:
            raise AssertionError(result.stderr + result.stdout)
        return result

    def props(service, name, credential):
        result = []
        for line in (root / 'deploy' / service).read_text().splitlines():
            if '=' not in line:
                continue
            key, value = line.split('=', 1)
            if key in ('Type','ExecStart','LoadCredential','StateDirectory','RuntimeDirectory','Restart','RestartSec','WantedBy','Description','After','Wants'):
                continue
            if key == 'User':
                value = name
            result.extend(['-p', f'{key}={value}'])
        result.extend(['-p',f'StateDirectory={name}','-p',f'LoadCredential=config.json:{credential}'])
        return result

    try:
        # Public test CA is copied outside /root so ProtectHome remains fully enabled.
        ca = Path('/run') / f'{agent_unit}.pem'
        shutil.copyfile(work/'ca.pem',ca)
        os.chmod(ca,0o644)
        agent_cfg = dict(cfg, state_dir=str(agent_state), ca_certificate=str(ca))
        agent_cfg_path = work/'sandbox-agent.json'
        agent_cfg_path.write_text(json.dumps(agent_cfg))
        # Fresh identity/ledger; register it against the still-running TLS panel.
        call(binary,'add-node','--config',work/'panel.json','--id','sandbox-live','--name','Sandbox verification',
             '--agent-config',work/'sandbox-generated.json','--interface',cfg['interfaces'][0])
        generated = json.loads((work/'sandbox-generated.json').read_text())
        agent_cfg.update(node_id='sandbox-live',token=generated['token'])
        agent_cfg_path.write_text(json.dumps(agent_cfg))
        args = ['systemd-run','--quiet','--wait','--collect','--service-type=exec',f'--unit={agent_unit}']
        args += props('needle-agent.service',agent_unit,agent_cfg_path)
        # systemd-run escapes literal percent specifiers; use the actual transient
        # credential path. Installed unit files use %d as shipped.
        args += [sandbox_binary,'agent','--config',f'/run/credentials/{agent_unit}.service/config.json','--once']
        result = call(*args,ok=False)
        if result.returncode:
            logs = call('journalctl','-u',agent_unit,'--no-pager','-n','30',ok=False)
            raise AssertionError(result.stderr + logs.stdout)
        code, body, _ = request('/api/nodes', viewer)
        node = next(n for n in json.loads(body)['nodes'] if n['id']=='sandbox-live')
        check('agent reports successfully under full DynamicUser/systemd sandbox',code==200 and node['online'])
        check('sandbox state owned by unprivileged identity',agent_state.stat().st_uid != 0)
        call(binary,'remove-node','--config',work/'panel.json','--id','sandbox-live')
        panel_cfg = json.loads((work/'panel.json').read_text())
        panel_cfg.update(database=str(panel_state/'panel.db'),socket=str(runtime/'http.sock'))
        panel_cfg_path = work/'sandbox-panel.json'
        panel_cfg_path.write_text(json.dumps(panel_cfg))
        args=['systemd-run','--quiet','--collect',f'--unit={panel_unit}']
        args+=props('needle-panel.service',panel_unit,panel_cfg_path)
        args+=['-p',f'RuntimeDirectory={panel_unit}',sandbox_binary,'serve','--config',f'/run/credentials/{panel_unit}.service/config.json']
        call(*args)
        targets.append(panel_unit)
        for _ in range(100):
            if (runtime/'http.sock').exists(): break
            time.sleep(.1)
        if not (runtime/'http.sock').exists():
            logs=call('journalctl','-u',panel_unit,'--no-pager','-n','30',ok=False)
            raise AssertionError(logs.stdout)
        origin=panel_cfg['origin'].split('://',1)[1]
        curl=call('curl','-sS','--unix-socket',runtime/'http.sock','-H',f'Host: {origin}',
                  '-H','X-Forwarded-Proto: https','-H',f'Authorization: Bearer {viewer}',
                  '-w','\n%{http_code}','http://localhost/api/nodes')
        check('panel serves authenticated API under AF_UNIX-only full sandbox',curl.stdout.endswith('\n200') and '"nodes":[]' in curl.stdout)
        check('panel database owned by unprivileged identity',(panel_state/'panel.db').stat().st_uid != 0)
        props_output=call('systemctl','show',panel_unit,'-p','MemoryMax','-p','NoNewPrivileges','-p','DynamicUser','-p','RestrictAddressFamilies').stdout
        (work/'sandbox.txt').write_text(props_output)
        print('SYSTEMD',props_output.strip().replace('\n','; '),flush=True)
        # Exercise the exact shipped administrator helper with isolated identities.
        admin_dir=work/'admin-config'
        admin_dir.mkdir()
        shutil.copyfile(panel_cfg_path,admin_dir/'panel.json')
        shutil.copyfile(agent_cfg_path,admin_dir/'agent.json')
        env=dict(os.environ,NEEDLE_BINARY=str(sandbox_binary),NEEDLE_CONFIG_DIR=str(admin_dir),
                 NEEDLE_PANEL_NAME=panel_unit,NEEDLE_AGENT_NAME=agent_unit)
        def admin(*args):
            r=subprocess.run(['bash',str(root/'deploy/needle-admin'),*args],env=env,text=True,capture_output=True)
            assert r.returncode == 0, r.stderr+r.stdout
        exported=work/'admin-enrollment.json'
        admin('add-node','admin-node','Admin helper',cfg['interfaces'][0],str(exported))
        check('shipped administrator helper enrolls node without root-owned database files',exported.exists() and len(json.loads(exported.read_text())['token'])==64)
        admin('remove-node','admin-node')
        admin('set-traffic','111111111','222222222')
        totals=json.loads((agent_state/'traffic.json').read_text())
        check('shipped helper imports historical totals under DynamicUser',totals['rx']==111111111 and totals['tx']==222222222)
    finally:
        for unit in targets:
            call('systemctl','stop',unit,ok=False)
        call('systemctl','reset-failed',panel_unit,agent_unit,ok=False)
        for name in [panel_unit,agent_unit]:
            # DynamicUser uses private backing directories and public symlinks.
            public=Path('/var/lib')/name
            if public.is_symlink(): public.unlink()
            elif public.exists(): shutil.rmtree(public)
            private=Path('/var/lib/private')/name
            if private.exists(): shutil.rmtree(private)
        if ca.exists(): ca.unlink()
        if sandbox_binary.exists(): sandbox_binary.unlink()
