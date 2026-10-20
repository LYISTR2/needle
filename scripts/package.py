#!/usr/bin/env python3
"""Generate static docs and a self-contained release archive; never package secrets."""
import hashlib
import html
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile

root = Path(__file__).resolve().parents[1]
docs = root / 'docs'
docs.mkdir(exist_ok=True)
readme = (root/'docs/guide.md').read_text()
security = (root/'SECURITY.md').read_text()
(docs/'llms.txt').write_text(readme + '\n\n' + security)
style = 'body{max-width:960px;margin:64px auto;padding:0 24px;background:#edf2f6;color:#263b4a;font:16px/1.8 system-ui}h1{letter-spacing:-1px}pre{white-space:pre-wrap;overflow-wrap:anywhere;font:14px/1.9 ui-monospace,monospace}a{color:#287d91}.palette{border:1px solid #d7e1e8;padding:24px;border-radius:12px;margin:24px 0}.swatches{display:flex;gap:8px;flex-wrap:wrap}.swatch{padding:20px;border-radius:6px;font:12px monospace}'
(docs/'index.html').write_text('<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Needle / 文档</title><style>'+style+'</style><h1>Needle / 只观测，不控制。</h1><nav><a href="llms.txt">纯文本 / AI</a> · <a href="themes.html">主题配色预览</a></nav><main><pre>'+html.escape(readme)+'</pre><h2>Security model</h2><pre>'+html.escape(security)+'</pre></main></html>')
palettes = {
    'Harbor / 雾港': ['#edf2f6','#ffffff','#263b4a','#627989','#287d91','#d7e1e8'],
    'Midnight / 深海': ['#111d29','#192a39','#e2edf2','#93adbc','#71c4d2','#304656'],
    'Paper / 纸墨': ['#f3f0e9','#fffcf5','#35382f','#74776a','#737d4a','#dddcd0'],
}
keys=['background','surface','text','muted','accent','line']
sections=[]
for name, colors in palettes.items():
    theme=dict(zip(keys,colors))
    sections.append(f'<section class="palette" style="background:{colors[0]};color:{colors[2]};border-color:{colors[5]}"><h2>{name}</h2><p style="color:{colors[3]}">主题仅改颜色，不加载第三方代码。这里是配色预览，不是实时服务器数据。</p><div class="swatches">'+''.join(f'<span class="swatch" style="background:{color};color:{colors[2] if i in [0,1,5] else colors[0]}">{key}<br>{color}</span>' for i,(key,color) in enumerate(theme.items()))+f'</div><pre>{html.escape(json.dumps(theme,indent=2))}</pre></section>')
(docs/'themes.html').write_text('<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Needle / 配色预览</title><style>'+style+'</style><h1>三种颜色，同一根针。</h1><p>把对应 JSON 写入 panel.json 的 theme 字段，再重启面板。</p><a href="index.html">返回文档</a>'+''.join(sections)+'</html>')

dist=root/'dist'
stage=dist/'needle-0.1.0-linux-x86_64'
if stage.exists(): shutil.rmtree(stage)
stage.mkdir(parents=True)
for item in ['Cargo.toml','Cargo.lock','README.md','SECURITY.md','LICENSE','src','web','deploy','docs','tests','scripts','evidence']:
    source=root/item
    if not source.exists(): continue
    if source.is_dir(): shutil.copytree(source,stage/item,ignore=shutil.ignore_patterns('__pycache__','*.pyc'))
    else: shutil.copyfile(source,stage/item)
shutil.copyfile(root/'target/release/needle',stage/'needle')
os.chmod(stage/'needle',0o755)
os.chmod(stage/'deploy/needle-admin',0o755)
manifest=[]
for file in sorted(stage.rglob('*')):
    if file.is_file(): manifest.append(f'{hashlib.sha256(file.read_bytes()).hexdigest()}  {file.relative_to(stage)}')
(stage/'SHA256SUMS').write_text('\n'.join(manifest)+'\n')
archive=dist/(stage.name+'.tar.gz')
with tarfile.open(archive,'w:gz') as tar: tar.add(stage,arcname=stage.name)
checksum=hashlib.sha256(archive.read_bytes()).hexdigest()
(dist/'SHA256SUMS').write_text(f'{checksum}  {archive.name}\n')
print(json.dumps({'archive':str(archive),'bytes':archive.stat().st_size,'sha256':checksum,'binary_bytes':(stage/'needle').stat().st_size},indent=2))
