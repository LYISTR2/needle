#!/usr/bin/env python3
"""Real local integration: release binary + isolated TLS nginx + live /proc data.
No production configuration, DNS, services or trust stores are modified.
Requires Linux, Python 3, openssl, nginx. Optional browser: NEEDLE_PLAYWRIGHT.
"""
import http.client
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import ssl
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/release/needle'
WORK = Path(tempfile.mkdtemp(prefix='needle-e2e-', dir=os.environ.get('TMPDIR')))
os.chmod(WORK, 0o755)
checks = []
processes = []


def run(*args, ok=True):
    result = subprocess.run([str(x) for x in args], text=True, capture_output=True)
    if ok and result.returncode:
        raise AssertionError(f'{args[0]} failed: {result.stderr}')
    return result


def check(name, condition):
    assert condition, name
    checks.append(name)
    print(f'PASS {name}', flush=True)


def unused_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


port = unused_port()
origin = f'https://localhost.localdomain:{port}'
panel_file = WORK / 'panel.json'
agent_file = WORK / 'agent.json'
viewer = None
context = None


def request(path, token=None, method='GET', data=None, host=None, extra=None):
    connection = http.client.HTTPSConnection('localhost.localdomain', port, context=context, timeout=10)
    headers = {'Host': host or f'localhost.localdomain:{port}'}
    if token:
        headers['Authorization'] = f'Bearer {token}'
    if data is not None:
        headers['Content-Type'] = 'application/json'
    headers.update(extra or {})
    connection.request(method, path, body=data, headers=headers)
    response = connection.getresponse()
    result = response.status, response.read(), dict(response.getheaders())
    connection.close()
    return result


def snapshot():
    code, payload, _ = request('/api/nodes', viewer)
    assert code == 200
    return json.loads(payload)


def start_panel():
    log = open(WORK / 'panel.log', 'a')
    p = subprocess.Popen([str(BINARY), 'serve', '--config', str(panel_file)], stdout=log, stderr=log)
    processes.append(p)
    for _ in range(100):
        if (WORK / 'http.sock').exists():
            return p
        if p.poll() is not None:
            raise AssertionError('panel exited: ' + (WORK / 'panel.log').read_text())
        time.sleep(.05)
    raise AssertionError('panel startup timeout')


try:
    init = run(BINARY, 'init', '--config', panel_file, '--origin', origin,
               '--database', WORK / 'panel.db', '--socket', WORK / 'http.sock')
    viewer = re.search(r'shown only now\): ([0-9a-f]{64})', init.stdout).group(1)
    interface = next(line.split(':')[0].strip() for line in Path('/proc/net/route').read_text().splitlines()[1:]
                     if line.split()[1] == '00000000')
    # /proc/net/route fields are tab separated; extract only interface.
    interface = interface.split()[0]
    run(BINARY, 'add-node', '--config', panel_file, '--id', 'local-live', '--name', 'Local · 实机采样',
        '--agent-config', agent_file, '--interface', interface)
    run('openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
        '-keyout', WORK / 'ca.key', '-out', WORK / 'ca.pem', '-subj', '/CN=Needle local test CA',
        '-addext', 'basicConstraints=critical,CA:TRUE',
        '-addext', 'keyUsage=critical,keyCertSign,cRLSign')
    run('openssl', 'req', '-new', '-newkey', 'rsa:2048', '-nodes',
        '-keyout', WORK / 'server.key', '-out', WORK / 'server.csr', '-subj', '/CN=localhost.localdomain')
    (WORK / 'server.ext').write_text('subjectAltName=DNS:localhost.localdomain\nbasicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\n')
    run('openssl', 'x509', '-req', '-in', WORK / 'server.csr', '-CA', WORK / 'ca.pem',
        '-CAkey', WORK / 'ca.key', '-CAcreateserial', '-out', WORK / 'server.pem', '-days', '1', '-extfile', WORK / 'server.ext')
    cfg = json.loads(agent_file.read_text())
    cfg.update(state_dir=str(WORK / 'agent-state'), ca_certificate=str(WORK / 'ca.pem'),
               targets=[{'name': 'local TLS', 'address': f'127.0.0.1:{port}'}])
    agent_file.write_text(json.dumps(cfg))
    context = ssl.create_default_context(cafile=str(WORK / 'ca.pem'))
    # Custom local-only nginx, not the host's running nginx instance.
    nginx_conf = f'''daemon off;
master_process off;
pid {WORK}/nginx.pid;
error_log {WORK}/nginx.log warn;
events {{ worker_connections 64; }}
http {{
  access_log off;
  client_body_temp_path {WORK}/client;
  proxy_temp_path {WORK}/proxy;
  server {{
    listen 127.0.0.1:{port} ssl;
    server_name localhost.localdomain;
    ssl_certificate {WORK}/server.pem;
    ssl_certificate_key {WORK}/server.key;
    client_max_body_size 16k;
    location / {{
      proxy_pass http://unix:{WORK}/http.sock;
      proxy_set_header Host $http_host;
      proxy_set_header X-Forwarded-Proto $scheme;
      proxy_set_header Connection "";
    }}
  }}
}}
'''
    (WORK / 'nginx.conf').write_text(nginx_conf)
    run('nginx', '-t', '-c', WORK / 'nginx.conf', '-p', WORK)
    panel = start_panel()
    nginx_log = open(WORK / 'nginx-console.log', 'w')
    nginx = subprocess.Popen(['nginx', '-c', str(WORK / 'nginx.conf'), '-p', str(WORK)], stdout=nginx_log, stderr=nginx_log)
    processes.append(nginx)
    status, body, headers = 0, b'', {}
    last_error = None
    for _ in range(100):
        try:
            status, body, headers = request('/')
            if status == 200:
                break
        except OSError as error:
            last_error = error
        time.sleep(.05)
    assert status == 200, f'HTTPS startup failed: {last_error}; status={status}'
    check('static HTML served through real HTTPS reverse proxy', status == 200 and b'PRIVATE MONITOR' in body)
    check('CSP and no-store headers', "default-src 'none'" in headers.get('content-security-policy', '') and headers.get('cache-control') == 'no-store')
    check('read API denies unauthenticated clients', request('/api/nodes')[0] == 401)
    check('read API denies agent credential', request('/api/nodes', cfg['token'])[0] == 401)
    check('rejects IP Host access', request('/', host=f'127.0.0.1:{port}')[0] == 403)
    check('rejects foreign Origin', request('/api/nodes', viewer, extra={'Origin':'https://evil.example'})[0] == 403)
    direct = run('curl', '-sS', '--unix-socket', WORK / 'http.sock', '-H', f'Host: localhost.localdomain:{port}', '-o', os.devnull, '-w', '%{http_code}', 'http://localhost/')
    check('Unix socket refuses missing HTTPS proxy header', direct.stdout == '403')
    direct_node = snapshot()['nodes'][0]
    check('registered node begins offline without synthetic metrics', not direct_node['online'] and direct_node['report'] is None)
    run(BINARY, 'agent', '--config', agent_file, '--once')
    first = snapshot()['nodes'][0]
    check('live agent submits CPU, memory, disk and network', first['online'] and first['report']['memory_total'] > 0 and first['report']['disk_total'] > 0)
    check('TCP latency measured against actual local listener', first['report']['latency'][0]['tcp_ms'] is not None)
    payload = json.dumps(first['report'])
    check('viewer credential cannot submit reports', request('/api/report/local-live', viewer, 'POST', payload)[0] == 401)
    check('replayed report rejected', request('/api/report/local-live', cfg['token'], 'POST', payload)[0] == 409)
    check('oversized body rejected', request('/api/report/local-live', cfg['token'], 'POST', 'x' * 17000)[0] == 413)
    bad = dict(first['report']); bad['sequence'] += 1; bad['cpu_percent'] = 101
    check('invalid metrics rejected', request('/api/report/local-live', cfg['token'], 'POST', json.dumps(bad))[0] == 422)
    bad = dict(first['report']); bad['sequence'] += 1
    check('per-node write rate limit', request('/api/report/local-live', cfg['token'], 'POST', json.dumps(bad))[0] == 429)
    time.sleep(2.1)
    run(BINARY, 'agent', '--config', agent_file, '--once')
    second = snapshot()['nodes'][0]['report']
    check('agent restart keeps monotonic cumulative traffic and sequence', second['rx_bytes'] >= first['report']['rx_bytes'] and second['tx_bytes'] >= first['report']['tx_bytes'] and second['sequence'] > first['report']['sequence'])
    run(BINARY, 'set-traffic', '--config', agent_file, '--rx', '123456789012', '--tx', '98765432100')
    saved = json.loads((WORK / 'agent-state/traffic.json').read_text())
    check('manual historical totals written exactly, sequence preserved', saved['rx'] == 123456789012 and saved['tx'] == 98765432100 and saved['sequence'] == second['sequence'])
    check('manual traffic edit creates backup', bool(list((WORK / 'agent-state').glob('traffic.backup-*.json'))))
    time.sleep(2.1)
    run(BINARY, 'agent', '--config', agent_file, '--once')
    third = snapshot()['nodes'][0]['report']
    check('historical traffic continues accumulating', third['rx_bytes'] >= 123456789012 and third['tx_bytes'] >= 98765432100)
    panel.terminate(); panel.wait(timeout=15)
    panel = start_panel()
    check('panel restart retains last metrics', snapshot()['nodes'][0]['report']['sequence'] == third['sequence'])
    # Verify locking with a real long-running agent.
    time.sleep(2.1)
    agent_log = open(WORK / 'agent.log', 'w')
    agent = subprocess.Popen([str(BINARY),'agent','--config',str(agent_file)], stdout=agent_log, stderr=agent_log)
    processes.append(agent)
    time.sleep(1.4)
    locked = run(BINARY, 'set-traffic', '--config', agent_file, '--rx', '0', '--tx', '0', ok=False)
    check('running agent prevents concurrent ledger mutation', locked.returncode != 0 and 'agent is running' in locked.stderr)
    status_text = Path(f'/proc/{agent.pid}/status').read_text()
    panel_status = Path(f'/proc/{panel.pid}/status').read_text()
    usage = {'agent_rss_kib': int(re.search(r'VmRSS:\s+(\d+)',status_text).group(1)),
             'panel_rss_kib': int(re.search(r'VmRSS:\s+(\d+)',panel_status).group(1)),
             'binary_bytes': BINARY.stat().st_size}
    print('MEASURED', json.dumps(usage), flush=True)
    if os.environ.get('NEEDLE_PLAYWRIGHT'):
        env = dict(os.environ, NEEDLE_TEST_ORIGIN=origin, NEEDLE_TEST_TOKEN=viewer,
                   NEEDLE_TEST_WORK=str(WORK), NODE_EXTRA_CA_CERTS=str(WORK/'ca.pem'))
        browser = subprocess.run(['node', str(ROOT/'tests/browser.cjs')], env=env, text=True, capture_output=True)
        print(browser.stdout, end='')
        assert browser.returncode == 0, browser.stderr
        checks.append('Chromium desktop/mobile authentication, data rendering and lock')
    agent.terminate(); agent.wait(timeout=20)
    # Corrupt ledgers must fail closed, not silently reset cumulative traffic.
    ledger_path = WORK / 'agent-state/traffic.json'
    good_ledger = ledger_path.read_bytes(); ledger_path.write_text('{broken')
    corrupt = run(BINARY,'agent','--config',agent_file,'--once',ok=False)
    check('corrupt ledger fails closed without replacement', corrupt.returncode != 0 and ledger_path.read_text() == '{broken')
    ledger_path.write_bytes(good_ledger)
    if os.environ.get('NEEDLE_SYSTEMD') == '1':
        from sandbox import verify
        verify(ROOT, WORK, BINARY, cfg, viewer, request, check)
    run(BINARY,'remove-node','--config',panel_file,'--id','local-live')
    check('node removal reflected in exact API readback', not snapshot()['nodes'])
    check('removed credential immediately revoked', request('/api/report/local-live',cfg['token'],'POST',payload)[0] == 401)
    check('no remote-command endpoint', request('/api/exec',viewer,'POST','{}')[0] == 404)
    evidence = {'passed':len(checks),'checks':checks,'measured':usage,'work_dir':str(WORK)}
    (WORK/'results.json').write_text(json.dumps(evidence,indent=2,ensure_ascii=False))
    print(f'COMPLETE {len(checks)} checks passed. Evidence: {WORK}/results.json', flush=True)
finally:
    for process in reversed(processes):
        if process.poll() is None:
            process.send_signal(signal.SIGTERM)
            try:
                process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                process.kill(); process.wait()
    print(f'Local test processes stopped; artifacts: {WORK}',flush=True)
