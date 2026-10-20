'use strict';
const $ = (id) => document.getElementById(id);
let credential = '', timer = null, busy = false, generation = 0;
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
  const fragment = document.createDocumentFragment();
  for (const node of data.nodes) {
    const card = $('node-template').content.cloneNode(true), article = card.querySelector('article'), r = node.report;
    article.classList.toggle('offline', !node.online);
    card.querySelector('.node-dot').classList.toggle('online', node.online);
    text(card,'.node-name',node.name);
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
  $('empty').hidden = data.nodes.length !== 0;
}
function lock() {
  generation++; credential = ''; samples = []; clearTimeout(timer); $('token').value = '';
  $('login').hidden = false; $('dashboard').hidden = true; $('logout').hidden = true;
  $('nodes').replaceChildren(); $('error').hidden = true;
  $('connection').textContent = '面板已锁定'; $('live-dot').classList.remove('online');
}
async function refresh() {
  if (busy || !credential) return;
  busy = true; const current = generation;
  try {
    const response = await fetch('./api/nodes', {headers:{Authorization:`Bearer ${credential}`},cache:'no-store',credentials:'omit',signal:AbortSignal.timeout(8000)});
    if (current !== generation) return;
    if (response.status === 401) { lock(); throw new Error('密钥无效。请使用初始化面板时生成的只读密钥，不是节点密钥。'); }
    if (!response.ok) throw new Error(`读取失败（HTTP ${response.status}），请检查反向代理配置。`);
    const data = await response.json();
    if (current !== generation) return;
    render(data); $('login').hidden = true; $('dashboard').hidden = false; $('logout').hidden = false;
    $('error').hidden = true; $('token').value = '';
    $('connection').textContent = '已同步 · 只读连接'; $('live-dot').classList.add('online');
  } catch (error) {
    if (current !== generation && credential) return;
    $('error').textContent = error.name === 'TimeoutError' ? '连接超时。保留上次数据，稍后自动重试。' : error.message;
    $('error').hidden = false; $('connection').textContent = '连接未建立 / 数据可能过期'; $('live-dot').classList.remove('online');
  } finally { busy = false; if (credential) timer = setTimeout(refresh, 10000); }
}
$('login-form').addEventListener('submit', event => {
  event.preventDefault(); if(busy) return;
  const value = $('token').value.trim();
  if (!/^[a-f0-9]{64}$/i.test(value)) { $('error').textContent = '请输入完整的 64 位十六进制密钥。'; $('error').hidden = false; return; }
  generation++; clearTimeout(timer); credential = value; refresh();
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
new ResizeObserver(drawChart).observe($('network-chart'));
$('theme').addEventListener('click',()=>requestAnimationFrame(drawChart));
$('logout').addEventListener('click',lock);
try { document.documentElement.dataset.theme = localStorage.getItem('needle-theme') || (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'); } catch {}
$('theme').addEventListener('click', () => { const value = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark'; document.documentElement.dataset.theme = value; try { localStorage.setItem('needle-theme',value); } catch {} });
