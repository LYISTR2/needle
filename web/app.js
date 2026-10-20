'use strict';
const $ = (id) => document.getElementById(id);
let credential = '', timer = null, busy = false, generation = 0;
let administrator = null, loginMode = 'admin', snapshot = null, editingId = null, selectedId = null;
let samples = [];
const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB', 'EiB'];
function bytes(value) { let n = Number(value), u = 0; while (n >= 1024 && u < units.length - 1) { n /= 1024; u++; } return `${n.toFixed(u ? 1 : 0)} ${units[u]}`; }
function percent(used, total) { return total ? used / total * 100 : 0; }
function duration(seconds) { const d = Math.floor(seconds / 86400), h = Math.floor(seconds % 86400 / 3600); return d ? `${d} 天 ${h} 时` : `${h} 时 ${Math.floor(seconds % 3600 / 60)} 分`; }
function text(root, selector, value) { root.querySelector(selector).textContent = value; }
function render(data) {
  $('online').textContent = `${data.nodes.filter(n => n.online).length} / ${data.nodes.length}`;
  $('total-rx').textContent = bytes(data.nodes.reduce((s,n) => s + (n.report?.rx_bytes || 0), 0));
  $('total-tx').textContent = bytes(data.nodes.reduce((s,n) => s + (n.report?.tx_bytes || 0), 0));
  $('updated').textContent = new Date(data.server_time * 1000).toLocaleTimeString('zh-CN', {hour12:false});
  $('node-count').textContent = data.nodes.length;
  const active = data.nodes.filter(n => n.online && n.report);
  const busiest = active.reduce((best,n) => !best || n.report.cpu_percent > best.report.cpu_percent ? n : best, null);
  $('busiest').textContent = busiest?.name || '—';
  $('busiest-detail').textContent = busiest ? `CPU 使用率 ${busiest.report.cpu_percent.toFixed(1)}%` : '暂无在线采样';
  const rx = active.reduce((sum,n) => sum+n.report.rx_bytes_per_second,0), tx = active.reduce((sum,n) => sum+n.report.tx_bytes_per_second,0);
  $('live-rx').textContent = `${bytes(rx)}/s`; $('live-tx').textContent = `${bytes(tx)}/s`;
  if (!samples.length || samples[samples.length-1].time !== data.server_time) samples.push({time:data.server_time,rx,tx});
  samples = samples.slice(-60); $('chart-status').textContent = `${samples.length} 个真实采样点`;
  requestAnimationFrame(drawChart);
  snapshot = data;
  updateGroups(data.nodes);
  const query = $('search').value.trim().toLowerCase(), group = $('group-filter').value, status = $('status-filter').value;
  const visible = data.nodes.filter(n => (!query || `${n.name} ${n.id} ${n.note} ${n.group_name}`.toLowerCase().includes(query)) && (!group || n.group_name === group) && (!status || n.online === (status === 'online')));
  $('filter-count').textContent = `${visible.length} / ${data.nodes.length} 台`;
  const fragment = document.createDocumentFragment();
  for (const node of visible) {
    const card = $('node-template').content.cloneNode(true), article = card.querySelector('article'), r = node.report;
    article.classList.toggle('offline', !node.online);
    card.querySelector('.node-dot').classList.toggle('online', node.online);
    text(card,'.node-name',node.name);
    text(card,'.group-tag',node.group_name || ''); card.querySelector('.group-tag').hidden = !node.group_name;
    text(card,'.node-note',node.note || ''); card.querySelector('.node-note').hidden = !node.note;
    text(card,'.expiry',node.expires_at ? `到期 ${node.expires_at}` : '');
    text(card,'.quota',node.quota_gib > 0 && r ? `配额 ${bytes(node.quota_gib * 1024 ** 3)} · 已用 ${((r.rx_bytes + r.tx_bytes) / (node.quota_gib * 1024 ** 3) * 100).toFixed(1)}%` : '');
    card.querySelector('.node-actions').hidden = !administrator;
    for (const button of card.querySelectorAll('[data-action]')) { button.dataset.id = node.id; button.setAttribute('aria-label', `${button.textContent} ${node.name}`); }
    text(card,'.node-status',node.online ? '运行中' : r ? '已离线' : '等待上报');
    text(card,'.node-meta',r ? `${r.os} · ${r.arch} · ${r.hostname}` : `节点 ${node.id} · 启动 agent 后自动显示数据`);
    for (const [name,value] of [['cpu',r?.cpu_percent],['memory',r && percent(r.memory_used,r.memory_total)],['disk',r && percent(r.disk_used,r.disk_total)]]) {
      text(card,`.${name}`,r ? `${value.toFixed(1)}%` : '—');
      card.querySelector(`.${name}-bar`).value = value || 0;
    }
    text(card,'.cpu-detail',r ? `${r.cpu_cores} vCPU · load ${r.load_1.toFixed(2)}` : '尚无数据');
    if (r) card.querySelector('.cpu-detail').title = r.cpu_model;
    text(card,'.memory-detail',r ? `${bytes(r.memory_used)} / ${bytes(r.memory_total)}` : '尚无数据');
    text(card,'.disk-detail',r ? `${bytes(r.disk_used)} / ${bytes(r.disk_total)}` : '尚无数据');
    for (const side of ['rx','tx']) {
      text(card,`.${side}-total`,r ? bytes(r[`${side}_bytes`]) : '—');
      text(card,`.${side}-speed`,r && node.online ? `${bytes(r[`${side}_bytes_per_second`])}/s` : '—');
    }
    const latency = card.querySelector('.latency');
    if (!r?.latency.length) { const tag = document.createElement('span'); tag.textContent = 'TCP 延迟 · 未配置目标'; latency.append(tag); }
    for (const p of r?.latency || []) { const tag = document.createElement('span'); tag.textContent = `${p.name} · ${p.tcp_ms === null ? '超时 / 不可达' : `${p.tcp_ms.toFixed(1)} ms`}`; latency.append(tag); }
    text(card,'.uptime',r ? `已运行 ${duration(r.uptime_seconds)}` : '等待首次采样');
    text(card,'.last-seen',node.received_at ? `${Math.max(0,data.server_time - node.received_at)} 秒前上报` : '—');
    fragment.append(card);
  }
  $('nodes').replaceChildren(fragment);
  $('empty').hidden = visible.length !== 0;
}
function lock() {
  generation++; credential = ''; administrator = null; snapshot = null; samples = []; clearTimeout(timer); $('token').value = '';
  for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close();
  $('install-command').textContent = ''; $('password-form').reset();
  $('login').hidden = false; $('dashboard').hidden = true; $('logout').hidden = true;
  $('add-node').hidden = true; $('password-button').hidden = true;
  $('nodes').replaceChildren(); $('error').hidden = true;
  $('connection').textContent = '等待登录'; $('live-dot').classList.remove('online');
}
async function api(path, options = {}) {
  const headers = {...options.headers};
  if (credential) headers.Authorization = `Bearer ${credential}`;
  if (options.body) headers['Content-Type'] = 'application/json';
  if (administrator && options.method && options.method !== 'GET') headers['X-CSRF-Token'] = administrator.csrf;
  const response = await fetch(`./api/${path}`, {cache:'no-store',credentials:'same-origin',...options,headers,signal:AbortSignal.timeout(15000)});
  if (!response.ok) {
    const message = await response.text();
    if (response.status === 401 && (administrator || credential)) lock();
    throw new Error(message || `请求失败（HTTP ${response.status}）`);
  }
  return response.status === 204 ? null : response.json();
}
async function refresh() {
  if (busy || (!credential && !administrator)) return;
  busy = true; const current = generation;
  try {
    const data = await api('nodes');
    if (current !== generation) return;
    $('login').hidden = true; $('dashboard').hidden = false; $('logout').hidden = false;
    $('add-node').hidden = !administrator; $('password-button').hidden = !administrator;
    render(data); $('error').hidden = true; $('token').value = '';
    $('connection').textContent = administrator ? `${administrator.username} · 管理员` : '只读访问'; $('live-dot').classList.add('online');
  } catch (error) {
    if (current !== generation) { if (!credential && !administrator) showError('error',error); return; }
    showError('error', error); $('connection').textContent = '连接中断 · 稍后重试'; $('live-dot').classList.remove('online');
  } finally { busy = false; if (credential || administrator) timer = setTimeout(refresh, 10000); }
}
$('login-form').addEventListener('submit', async event => {
  event.preventDefault(); if (busy) return;
  $('login-submit').disabled = true;
  try {
    if (loginMode === 'viewer') {
      const value = $('token').value.trim();
      if (!/^[a-f0-9]{64}$/i.test(value)) throw new Error('请输入完整的 64 位只读密钥。');
      credential = value; administrator = null;
    } else {
      administrator = await api('auth/login', {method:'POST',body:JSON.stringify({username:$('username').value,password:$('token').value})});
      credential = '';
    }
    generation++; clearTimeout(timer); await refresh();
  } catch (error) { showError('error',error); }
  finally { $('login-submit').disabled = false; }
});
function drawChart() {
  const canvas = $('network-chart'), box = canvas.getBoundingClientRect();
  if (!box.width || !box.height) return;
  const ratio = devicePixelRatio || 1;
  canvas.width = Math.round(box.width*ratio); canvas.height = Math.round(box.height*ratio);
  const ctx = canvas.getContext('2d'); ctx.scale(ratio,ratio);
  const css = getComputedStyle(document.documentElement), color = key => css.getPropertyValue(key).trim();
  const left=65, top=10, right=12, bottom=22, w=box.width-left-right, h=box.height-top-bottom;
  const maximum=Math.max(1024,...samples.flatMap(s=>[s.rx,s.tx]))*1.15;
  ctx.font='9px sans-serif'; ctx.lineWidth=1;
  for(let i=0;i<4;i++) { const y=top+h*i/3; ctx.strokeStyle=color('--line'); ctx.beginPath();ctx.moveTo(left,y);ctx.lineTo(left+w,y);ctx.stroke();ctx.fillStyle=color('--muted');ctx.fillText(`${bytes(maximum*(1-i/3))}/s`,0,y+3); }
  if(!samples.length) return;
  const start=samples[0].time, span=Math.max(10,samples[samples.length-1].time-start);
  for(const [key,tint] of [['rx','--green'],['tx','--purple']]) {
    ctx.strokeStyle=color(tint);ctx.fillStyle=color(tint);ctx.lineWidth=1.7;ctx.beginPath();
    samples.forEach((s,i)=>{const x=left+(s.time-start)/span*w,y=top+h-s[key]/maximum*h;i?ctx.lineTo(x,y):ctx.moveTo(x,y);});ctx.stroke();
    if(samples.length===1){ctx.beginPath();ctx.arc(left,top+h-samples[0][key]/maximum*h,2.5,0,Math.PI*2);ctx.fill();}
  }
  ctx.fillStyle=color('--muted'); const stamp=t=>new Date(t*1000).toLocaleTimeString('zh-CN',{hour12:false});
  ctx.fillText(stamp(start),left,box.height-2);ctx.textAlign='right';ctx.fillText(stamp(samples[samples.length-1].time),left+w,box.height-2);
}
function showError(id,error) { $(id).textContent = error.name === 'TimeoutError' ? '连接超时，请稍后重试。' : error.message; $(id).hidden = false; }
function notify(message) { $('notice').textContent=message; $('notice').hidden=false; setTimeout(()=>$('notice').hidden=true,5000); }
function updateGroups(nodes) {
  const selected=$('group-filter').value, groups=[...new Set(nodes.map(n=>n.group_name).filter(Boolean))].sort();
  const options=[new Option('全部分组',''),...groups.map(g=>new Option(g,g))];
  $('group-filter').replaceChildren(...options); $('group-filter').value=groups.includes(selected)?selected:'';
}
function switchMode(mode) {
  loginMode=mode; $('username-row').hidden=mode==='viewer'; $('username').required=mode==='admin';
  $('password-label').textContent=mode==='admin'?'密码':'面板只读密钥'; $('token').value='';
  $('token').autocomplete=mode==='admin'?'current-password':'off'; $('token').placeholder=mode==='admin'?'输入管理员密码':'输入 64 位只读密钥';
  $('login-note').textContent=mode==='admin'?'登录状态有效期 12 小时，可在右上角修改密码。':'只读密钥仅保存在当前页面内存，无法管理节点。';
  $('admin-tab').classList.toggle('selected',mode==='admin'); $('viewer-tab').classList.toggle('selected',mode==='viewer'); $('error').hidden=true;
}
$('admin-tab').addEventListener('click',()=>switchMode('admin')); $('viewer-tab').addEventListener('click',()=>switchMode('viewer'));
for(const id of ['search','group-filter','status-filter']) $(id).addEventListener('input',()=>{if(snapshot) render(snapshot);});
for(const button of document.querySelectorAll('[data-close]')) button.addEventListener('click',()=>$(button.dataset.close).close());
$('install-dialog').addEventListener('close',()=>{$('install-command').textContent='';});
function openEditor(node=null) {
  editingId=node?.id||null; $('node-form').reset(); $('node-error').hidden=true;
  $('node-id').value=node?.id||''; $('node-id').disabled=!!node;
  for(const [id,key] of [['node-name','name'],['node-group','group_name'],['node-note','note'],['node-expiry','expires_at'],['node-quota','quota_gib'],['node-order','sort_order']]) $(id).value=node?.[key] ?? (id==='node-quota'||id==='node-order'?0:'');
  $('node-interface-row').hidden=!!node; $('node-interface').required=!node;
  $('node-dialog-title').textContent=node?'编辑服务器':'添加服务器'; $('node-dialog').showModal();
}
$('add-node').addEventListener('click',()=>openEditor());
function showInstall(result) { $('install-command').textContent=result.command; $('install-note').textContent=result.notice; $('copy-command').textContent='复制安装命令'; $('install-dialog').showModal(); }
$('node-form').addEventListener('submit',async event=>{
  event.preventDefault(); const submit=event.submitter; submit.disabled=true;
  try {
    const input={id:$('node-id').value,name:$('node-name').value,group_name:$('node-group').value,note:$('node-note').value,expires_at:$('node-expiry').value,quota_gib:Number($('node-quota').value),sort_order:Number($('node-order').value),interface:$('node-interface').value};
    const result=await api(editingId?`nodes/${encodeURIComponent(editingId)}`:'nodes',{method:editingId?'PUT':'POST',body:JSON.stringify(input)});
    $('node-dialog').close(); notify(editingId?'节点已保存':'节点已创建'); if(result) showInstall(result); clearTimeout(timer); await refresh();
  }catch(error){showError('node-error',error);}finally{submit.disabled=false;}
});
$('nodes').addEventListener('click',event=>{
  const button=event.target.closest('[data-action]'); if(!button||!administrator) return;
  const node=snapshot.nodes.find(n=>n.id===button.dataset.id); if(!node)return; selectedId=node.id;
  if(button.dataset.action==='edit')openEditor(node);
  if(button.dataset.action==='enroll') { $('enroll-node').textContent=node.name; $('enroll-error').hidden=true; $('enroll-form').reset(); $('enroll-dialog').showModal(); }
  if(button.dataset.action==='delete') { $('delete-description').textContent=`确定删除「${node.name}」？此操作无法撤销。`; $('delete-error').hidden=true; $('delete-dialog').showModal(); }
});
$('enroll-form').addEventListener('submit',async event=>{
  event.preventDefault(); event.submitter.disabled=true;
  try { const result=await api(`nodes/${encodeURIComponent(selectedId)}/enroll`,{method:'POST',body:JSON.stringify({interface:$('enroll-interface').value})}); $('enroll-dialog').close(); showInstall(result); }
  catch(error){showError('enroll-error',error);}finally{event.submitter.disabled=false;}
});
$('confirm-delete').addEventListener('click',async()=>{
  $('confirm-delete').disabled=true;
  try {await api(`nodes/${encodeURIComponent(selectedId)}`,{method:'DELETE'});$('delete-dialog').close();notify('节点已删除，凭据已撤销');clearTimeout(timer);await refresh();}
  catch(error){showError('delete-error',error);}finally{$('confirm-delete').disabled=false;}
});
$('copy-command').addEventListener('click',async()=>{
  try{await navigator.clipboard.writeText($('install-command').textContent);$('copy-command').textContent='已复制';}
  catch{const range=document.createRange();range.selectNodeContents($('install-command'));const selection=getSelection();selection.removeAllRanges();selection.addRange(range);$('copy-command').textContent='请复制选中的命令';}
});
$('password-button').addEventListener('click',()=>{$('password-form').reset();$('password-error').hidden=true;$('password-dialog').showModal();});
$('password-form').addEventListener('submit',async event=>{
  event.preventDefault(); event.submitter.disabled=true;
  try {if($('new-password').value!==$('repeat-password').value)throw new Error('两次输入的新密码不一致');await api('auth/password',{method:'POST',body:JSON.stringify({current_password:$('current-password').value,new_password:$('new-password').value})});lock();notify('密码已更新，请重新登录');}
  catch(error){showError('password-error',error);}finally{event.submitter.disabled=false;}
});
(async()=>{try{administrator=await api('auth/session');await refresh();}catch{administrator=null;}})();
new ResizeObserver(drawChart).observe($('network-chart'));
$('theme').addEventListener('click',()=>requestAnimationFrame(drawChart));
$('logout').addEventListener('click',async () => {
  try { if (administrator) await api('auth/logout',{method:'POST'}); lock(); }
  catch(error) { showError('error',error); }
});
try { document.documentElement.dataset.theme = localStorage.getItem('needle-theme') || (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'); } catch {}
$('theme').addEventListener('click', () => { const value = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark'; document.documentElement.dataset.theme = value; try { localStorage.setItem('needle-theme',value); } catch {} });
