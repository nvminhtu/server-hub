// Server Hub UI: "Localhost" = every open port with a health check; "Projects" = everything that can run, grouped by repo.
// Poll the core every 2.5 s; the selected row's log every second while it runs.
import './styles.css';
import { call, isTauri, type Port, type Row, type Snapshot } from './api';

if (isTauri) document.documentElement.classList.add('tauri');

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const esc = (s: unknown) => String(s ?? '').replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]!);

const LOCAL = '__localhost__';
let snap: Snapshot = { root: '', roots: [], rootExists: true, entries: [], outside: [], ports: [], unhealthy: 0, running: 0, lanIp: null };
let view = LOCAL; // LOCAL | 'all' | `g:<group>` | `r:<repo>` | `p:<repo>/<project>`
const SEP = '\u0001';
const store = {
  get: (k: string) => { try { return localStorage.getItem(k); } catch { return null; } },
  set: (k: string, v: string) => { try { localStorage.setItem(k, v); } catch { /* private window */ } },
};
const collapsed = new Set<string>(JSON.parse(store.get('hub.collapsed') || '[]'));
view = store.get('hub.view') || LOCAL;
if (view === '__outside__') view = LOCAL;
function setView(v: string) { view = v; store.set('hub.view', v); }
const inView = (r: Row) =>
  view === 'all' || view === `g:${r.group}` || view === `r:${r.repo}` || view === `p:${r.repo}${SEP}${r.project}`;
const isOn = (r: Row) => r.status === 'running' || r.status === 'starting';
// Default: only what runs (list + sidebar). "Show all" brings back every stopped project; the choice is remembered.
let showAll = store.get('hub.showAll') === '1';
// a stopped row stays visible while it is selected or busy, so ▶ is still there right after ■
const shown = (r: Row) => showAll || isOn(r) || r.id === selected || busy.has(r.id) || r.status === 'error';
let selected: string | null = null; // row id, or `port:<n>` in the localhost view
let armed: string | null = null; // stop button waiting for a second click
let busy = new Set<string>();

function ago(s: number | null) {
  if (s == null) return '';
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

function toast(msg: string, bad = false) {
  const t = $('toast');
  t.textContent = msg;
  t.className = 'toast show' + (bad ? ' bad' : '');
  clearTimeout((t as any)._h);
  (t as any)._h = setTimeout(() => (t.className = 'toast'), 3500);
}

async function refresh(rescan = false) {
  try {
    snap = await call<Snapshot>('snapshot', { rescan });
    if (view !== 'all' && view !== LOCAL && !snap.entries.some((r) => inView(r) && shown(r))) setView(snap.entries.length ? 'all' : LOCAL);
    render();
  } catch (e: any) {
    $('list').innerHTML = `<div class="empty">Can't reach the Server Hub core.<br><span class="mono faint">${esc(e.message)}</span><br><br>In a browser, start it with <span class="mono">npm run bridge</span>.</div>`;
  }
}

function matches(r: { name: string; command?: string; cwd: string; port?: number | null; ports: number[]; args?: string }) {
  const q = ($<HTMLInputElement>('q').value || '').trim().toLowerCase();
  if (!q) return true;
  const hay = [r.name, r.command, r.cwd, r.args, r.port, (r as any).project, (r as any).repo, ...r.ports].join(' ').toLowerCase();
  return q.split(/\s+/).every((w) => hay.includes(w.replace(/^:/, '')));
}

function visibleRows(): Row[] {
  return snap.entries.filter((r) => inView(r) && matches(r) && shown(r));
}

function renderSide() {
  type G = { n: number; on: number };
  const count = (rs: Row[]): G => ({ n: rs.length, on: rs.filter(isOn).length });
  const item = (key: string, label: string, g: G, cls = '', extra = '') =>
    `<button class="si ${view === key ? 'sel' : ''} ${cls}" data-view="${esc(key)}">${extra}<span class="sd ${g.on ? 'on' : ''}"></span><span class="sl" title="${esc(label)}">${esc(label)}</span><span class="sc">${g.on ? `<b>${g.on}</b>/` : ''}${g.n}</span></button>`;
  const bad = snap.unhealthy;
  let h = `<button class="si ${view === LOCAL ? 'sel' : ''} local" data-view="${LOCAL}"><span class="sd ${snap.ports.length ? (bad ? 'bad' : 'on') : ''}"></span><span class="sl">Localhost</span><span class="sc">${bad ? `<i>${bad}!</i> ` : ''}${snap.ports.length}</span></button>`;
  h += '<div class="sh">Projects</div>';
  if (!snap.entries.length) {
    h += `<div class="nope faint">${snap.rootExists ? 'No runnable project found yet.' : 'Pick the folder(s) where your projects live.'}</div>`;
    h += `<button class="pick" data-root="add">+ Add projects folder…</button>`;
    $('side').innerHTML = h + folders();
    return;
  }
  h += item('all', showAll ? 'All projects' : 'Running projects', count(snap.entries));
  // group (top folder) → repo → project; projects only open for the repo you are looking at
  const vis = snap.entries.filter(shown);
  if (!vis.length) h += `<div class="nope faint">Nothing running. Tick <b>Show all</b> to see all ${snap.entries.length}.</div>`;
  const groups = [...new Set(vis.map((r) => r.group))];
  const byRun = (a: [string, Row[]], b: [string, Row[]]) => count(b[1]).on - count(a[1]).on || a[0].localeCompare(b[0]);
  const gRows = groups.map((g) => [g, vis.filter((r) => r.group === g)] as [string, Row[]]).sort(byRun);
  for (const [g, rows] of gRows) {
    const shut = collapsed.has(g);
    h += `<div class="gr">${item(`g:${g}`, g, count(rows), 'grp', `<span class="tw" data-fold="${esc(g)}">${shut ? '▸' : '▾'}</span>`)}</div>`;
    if (shut) continue;
    const repos = [...new Set(rows.map((r) => r.repo))].map((x) => [x, rows.filter((r) => r.repo === x)] as [string, Row[]]).sort(byRun);
    for (const [repo, rr] of repos) {
      h += item(`r:${repo}`, repo, count(rr), 'lv1');
      const projects = [...new Set(rr.map((r) => r.project))];
      const open = view === `r:${repo}` || view.startsWith(`p:${repo}${SEP}`);
      if (open && projects.length > 1) {
        for (const p of projects.sort()) h += item(`p:${repo}${SEP}${p}`, p, count(rr.filter((r) => r.project === p)), 'lv2');
      }
    }
  }
  $('side').innerHTML = h + folders();
}

// Folders that get scanned for projects: add as many as you like, × to drop one.
function folders() {
  const list = snap.roots.filter((r) => r.exists || snap.roots.length > 1 || snap.entries.length);
  return `<div class="roots"><div class="sh">Folders</div>${list
    .map((r) => `<div class="rt mono ${r.exists ? '' : 'gone'}" title="${esc(r.path)}${r.exists ? '' : ' (not found)'}"><span class="rp">&lrm;${esc(r.path)}&lrm;</span><button class="rx ${armed === 'root:' + r.path ? 'armed' : ''}" data-unroot="${esc(r.path)}" title="Stop scanning this folder (nothing is deleted)">${armed === 'root:' + r.path ? 'remove?' : '×'}</button></div>`)
    .join('')}<button class="addroot" data-root="add" title="Scan another folder for projects">+ Add folder</button></div>`;
}

function actions(r: Row) {
  const on = r.status === 'running' || r.status === 'starting';
  const b = busy.has(r.id);
  let h = '';
  if (on) {
    const isArmed = armed === r.id;
    h += `<button class="act stop ${isArmed ? 'armed' : ''}" data-stop="${r.id}" title="${r.own ? 'Stop' : `Stop (started by ${esc(r.source)})`}">${isArmed ? 'Stop?' : '■'}</button>`;
  } else if (r.canStart) {
    h += `<button class="act play" data-start="${r.id}" ${b ? 'disabled' : ''} title="Start">${b ? '…' : '▶'}</button>`;
  } else {
    h += `<span class="act ghost" title="Attach-only: Claude preview opens this URL, nothing to start">—</span>`;
  }
  if (r.url && (on || !r.canStart)) h += `<button class="act" data-open="${esc(r.url)}" title="${esc(r.url)}">↗</button>`;
  else h += `<span class="act ghost"></span>`;
  const lp = on ? (r.ports[0] ?? null) : null;
  h += lp ? shareBtn(lp) : `<span class="act ghost"></span>`;
  h += `<button class="act" data-folder="${esc(r.cwdFull)}" title="Open folder in Finder">⌂</button>`;
  h += `<button class="act" data-copy="${esc(`cd ${r.cwd} && ${r.command}`)}" title="Copy command">⧉</button>`;
  return h;
}

// ⇄ = share on the local network. Green when other devices can open it, with the link in the tooltip.
function shareBtn(port: number) {
  const p = snap.ports.find((x) => x.port === port);
  if (!p) return `<span class="act ghost"></span>`;
  const key = `share:${port}`;
  if (p.lan === 'shared') {
    const isArmed = armed === key;
    return `<button class="act lan on ${isArmed ? 'armed' : ''}" data-unshare="${port}" title="Shared on the LAN: ${esc(p.lanUrl)}\nClick to copy, click Stop? to stop sharing">${isArmed ? 'Stop?' : '⇄'}</button>`;
  }
  if (p.lan === 'open') return `<button class="act lan on" data-share="${port}" title="Already reachable on the LAN: ${esc(p.lanUrl)} (click to copy)">⇄</button>`;
  return `<button class="act lan" data-share="${port}" title="${snap.lanIp ? 'Share on the local network: other devices on the same Wi-Fi can open it' : 'Not on a local network'}" ${snap.lanIp ? '' : 'disabled'}>⇄</button>`;
}

const lanTag = (port?: number) => { const p = snap.ports.find((x) => x.port === port); return p?.lanUrl ? `LAN ${p.lanUrl.replace('http://', '')}` : ''; };

function rowHtml(r: Row) {
  const port = r.ports.length ? r.ports.map((p) => `:${p}`).join(' ') : r.port ? `:${r.port}` : r.kind === 'run.sh' ? 'game' : '';
  const meta = r.status === 'running' || r.status === 'starting' ? [lanTag(r.ports[0]), ago(r.secs), r.mb != null ? `${r.mb} MB` : '', r.own ? '' : r.source ? `by ${r.source}` : ''].filter(Boolean).join(' · ') : '';
  const warn = r.note ? `<span class="chip ${r.status === 'error' ? 'red' : 'red'}" title="${esc(r.note)}">${esc(r.note)}</span>` : r.sharedPort.length && r.status !== 'running' ? `<span class="chip amber" title="Same port as ${esc(r.sharedPort.join(', '))}">shares :${r.port}</span>` : '';
  return `<div class="row ${selected === r.id ? 'sel' : ''} st-${r.status}" data-row="${r.id}">
    <span class="dot ${r.status}" title="${r.status}"></span>
    <span class="nm mono">${esc(r.name)}</span>
    <span class="cmd mono" title="${esc(r.cwd)}\n${esc(r.command)}">${esc(r.command)}</span>
    <span class="port mono">${esc(port)}</span>
    <span class="meta">${warn || esc(meta)}</span>
    <span class="acts">${actions(r)}</span>
  </div>`;
}

function renderList() {
  if (view === LOCAL) return renderPorts();
  const rows = visibleRows();
  if (!rows.length) {
    const hidden = snap.entries.filter((r) => inView(r) && matches(r) && !shown(r)).length;
    $('list').innerHTML = `<div class="empty">${!snap.entries.length ? 'Scanning…' : hidden ? `No project server is running here.<br><span class="faint">${hidden} stopped — tick <b>Show all</b> to see and start them.</span>` : 'Nothing matches.'}</div>`;
    return;
  }
  const key = (r: Row) => (view.startsWith('r:') ? r.project : view.startsWith('p:') ? '' : r.repo);
  const groups = new Map<string, Row[]>();
  for (const r of rows) groups.set(key(r), [...(groups.get(key(r)) ?? []), r]);
  let h = '';
  for (const [title, rs] of groups) {
    const on = rs.filter((r) => r.status === 'running').length;
    if (title) h += `<div class="gh"><span>${esc(title)}</span><span class="faint">${on ? `${on} running · ` : ''}${rs.length}</span></div>`;
    h += rs.map(rowHtml).join('');
  }
  $('list').innerHTML = h;
}

const HEALTH: Record<string, string> = { healthy: 'healthy', unhealthy: 'unhealthy', tcp: 'open · not a web page' };
function healthText(p: Port) {
  if (p.health === 'healthy') return `HTTP ${p.code}${p.ms != null ? ` · ${p.ms} ms` : ''}`;
  if (p.health === 'unhealthy') return p.code ? `HTTP ${p.code}` : 'no answer';
  return 'no HTTP reply';
}

function renderPorts() {
  const rows = snap.ports.filter((p) => matches({ ...p, command: `${p.tool} ${p.args}`, ports: [p.port] }));
  if (!rows.length) {
    $('list').innerHTML = `<div class="empty">${snap.ports.length ? 'Nothing matches.' : 'No localhost port is open.<br><span class="faint">Start a dev server (npm run dev, python -m http.server…) and it shows up here within 3 seconds.</span>'}</div>`;
    return;
  }
  const bad = rows.filter((p) => p.health === 'unhealthy').length;
  let h = `<div class="gh"><span>Open on localhost</span><span class="faint">${bad ? `<b class="redt">${bad} unhealthy</b> · ` : ''}${rows.length}</span></div>`;
  for (const p of rows) {
    const key = `port:${p.port}`;
    const isArmed = armed === key;
    h += `<div class="row prow ${selected === key ? 'sel' : ''} h-${p.health}" data-row="${key}">
      <span class="dot ${p.health}" title="${HEALTH[p.health]}"></span>
      <span class="port big mono">:${p.port}</span>
      <span class="nm mono" title="pid ${p.pid} · ${esc(p.cwd)}">${esc(p.name)} <span class="tool">${esc(p.tool)}</span></span>
      <span class="hl ${p.health}">${esc(healthText(p))}</span>
      <span class="meta">${esc([p.lanUrl ? `LAN ${p.lanUrl.replace('http://', '')}` : '', ago(p.secs), `${p.mb} MB`, p.source ? `by ${p.source}` : ''].filter(Boolean).join(' · '))}</span>
      <span class="acts">
        <button class="act" data-open="http://localhost:${p.port}" title="Open http://localhost:${p.port}">↗</button>
        ${p.health === 'tcp' ? '<span class="act ghost"></span>' : shareBtn(p.port)}
        <button class="act stop ${isArmed ? 'armed' : ''}" data-kill="${p.port}" title="Stop pid ${p.pid}">${isArmed ? 'Stop?' : '■'}</button>
        ${p.cwd && p.cwd !== '/' ? `<button class="act" data-folder="${esc(p.cwd)}" title="Open folder in Finder">⌂</button>` : '<span class="act ghost"></span>'}
        <button class="act" data-copy="http://localhost:${p.port}" title="Copy URL">⧉</button>
      </span>
    </div>`;
  }
  $('list').innerHTML = h;
}

function render() {
  $('pill').textContent = `${snap.ports.length} on localhost`;
  $('pill').classList.toggle('on', snap.ports.length > 0);
  $('bad').textContent = snap.unhealthy ? `${snap.unhealthy} unhealthy` : '';
  $('bad').style.display = snap.unhealthy ? '' : 'none';
  $<HTMLInputElement>('showAll').checked = showAll;
  $('rescan').title = view === LOCAL ? 'Check every port again' : 'Scan the projects folder again';
  const n = stopTargets().length;
  $<HTMLButtonElement>('stopAll').disabled = n === 0;
  $('stopAll').textContent = n ? `■ Stop all (${n})` : '■ Stop all';
  renderSide();
  renderList();
  renderLogHead();
}

// ---- log pane -------------------------------------------------------------------------------------------------
let logTimer = 0;
function renderLogHead() {
  const r = snap.entries.find((e) => e.id === selected);
  const o = snap.ports.find((x) => `port:${x.port}` === selected);
  if (r) {
    $('logTitle').innerHTML = `<span class="dot ${r.status}"></span><b class="mono">${esc(r.name)}</b> <span class="faint mono" title="from ${esc(r.file)}">${esc(r.cwd)}</span>`;
  } else if (o) {
    $('logTitle').innerHTML = `<span class="dot ${o.health}"></span><b class="mono">:${o.port} ${esc(o.name)}</b> <span class="faint mono">pid ${o.pid} · ${esc(o.cwd)}</span>`;
  } else {
    $('logTitle').textContent = 'Select a row to see its log';
  }
}

async function loadLog() {
  const pre = $('log');
  const o = snap.ports.find((x) => `port:${x.port}` === selected);
  const r = snap.entries.find((e) => e.id === (o?.rowId ?? selected));
  if (!r && !o) {
    pre.textContent = '';
    $('logHint').textContent = '↑↓ select · Space start/stop · ↵ open · ⌘K search';
    return;
  }
  if (o && !r?.own) {
    pre.textContent = `$ ${o.args}\n  in ${o.cwd || '?'}\n\nhttp://localhost:${o.port} → ${HEALTH[o.health]} (${healthText(o)})\nStarted by ${o.source || '?'} · pid ${o.pid} · ${o.mb} MB · up ${ago(o.secs)}\n\nIts log lives in the window that started it — Server Hub only keeps logs of servers it started itself (Projects → ▶).`;
    $('logHint').textContent = '↑↓ select · ↵ open · Space stop · ⌘K search';
    return;
  }
  const l = await call<{ lines: string[]; own: boolean }>('logs', { id: r!.id }).catch(() => ({ lines: [], own: false }));
  if (l.lines.length) {
    const atBottom = pre.scrollHeight - pre.scrollTop - pre.clientHeight < 30;
    pre.innerHTML = l.lines.map((s) => `<span class="${/error|fail|exception|EADDRINUSE/i.test(s) ? 'lr' : /ready|local:|listening|started|compiled/i.test(s) ? 'lg' : ''}">${esc(s)}</span>`).join('\n');
    if (atBottom) pre.scrollTop = pre.scrollHeight;
    $('logHint').textContent = `${l.lines.length} lines`;
  } else if (r!.status === 'running') {
    pre.textContent = `$ ${r!.command}\n\nRunning, started by ${r!.source || 'another app'} (pid ${r!.pid}). Its log lives there — stop it and press ▶ here to see the log in Server Hub.`;
    $('logHint').textContent = '';
  } else {
    pre.textContent = `$ ${r!.command}\n  in ${r!.cwd}\n\nPress ▶ (or Space) to start.`;
    $('logHint').textContent = '';
  }
}

function select(key: string | null) {
  selected = key;
  armed = null;
  renderList();
  renderLogHead();
  loadLog();
  document.querySelector('.row.sel')?.scrollIntoView({ block: 'nearest' });
}

// ---- actions ----------------------------------------------------------------------------------------------
async function start(id: string) {
  busy.add(id);
  renderList();
  try {
    await call('start', { id });
    select(id);
  } catch (e: any) {
    toast(e.message, true);
  }
  busy.delete(id);
  await refresh();
  setTimeout(refresh, 1200);
}

async function stop(id: string) {
  if (armed !== id) {
    armed = id;
    renderList();
    setTimeout(() => { if (armed === id) { armed = null; renderList(); } }, 3000);
    return;
  }
  armed = null;
  try {
    const p = id.startsWith('port:') ? snap.ports.find((x) => `port:${x.port}` === id) : null;
    if (p?.rowId) await call('stop', { id: p.rowId });
    else if (p) await call('kill', { pid: p.pid });
    else await call('stop', { id });
    toast('Stopping…');
  } catch (e: any) {
    toast(e.message, true);
  }
  setTimeout(refresh, 400);
  setTimeout(refresh, 3500);
}

function toggle(key: string) {
  const r = snap.entries.find((e) => e.id === key);
  if (r) {
    if (r.status === 'running' || r.status === 'starting') stop(r.id);
    else if (r.canStart) start(r.id);
  } else if (key.startsWith('port:')) stop(key);
}

async function open(target: string, app?: string) {
  await call('open', { target, app }).catch((e) => toast(e.message, true));
}

document.addEventListener('click', (ev) => {
  const t = ev.target as HTMLElement;
  const fold = t.closest<HTMLElement>('[data-fold]');
  if (fold) {
    ev.stopPropagation();
    const g = fold.dataset.fold!;
    if (collapsed.has(g)) collapsed.delete(g); else collapsed.add(g);
    store.set('hub.collapsed', JSON.stringify([...collapsed]));
    renderSide();
    return;
  }
  if (t.closest('[data-root]')) {
    ev.stopPropagation();
    addRoot();
    return;
  }
  const un = t.closest<HTMLElement>('[data-unroot]');
  if (un) {
    ev.stopPropagation();
    removeRoot(un.dataset.unroot!);
    return;
  }
  const b = t.closest<HTMLElement>('[data-start],[data-stop],[data-kill],[data-open],[data-folder],[data-copy],[data-view],[data-share],[data-unshare]');
  if (b) {
    ev.stopPropagation();
    const d = b.dataset;
    if (d.view != null) { setView(d.view); selected = null; render(); loadLog(); }
    else if (d.start) start(d.start);
    else if (d.stop) stop(d.stop);
    else if (d.kill) stop(`port:${d.kill}`);
    else if (d.share) share(Number(d.share));
    else if (d.unshare) unshare(Number(d.unshare));
    else if (d.open) open(d.open);
    else if (d.folder) open(d.folder);
    else if (d.copy) navigator.clipboard.writeText(d.copy).then(() => toast('Copied: ' + d.copy));
    return;
  }
  const row = t.closest<HTMLElement>('[data-row]');
  if (row) select(row.dataset.row!);
});

document.addEventListener('dblclick', (ev) => {
  const row = (ev.target as HTMLElement).closest<HTMLElement>('[data-row]');
  if (row && !(ev.target as HTMLElement).closest('button')) toggle(row.dataset.row!);
});

document.addEventListener('keydown', (ev) => {
  const inInput = (ev.target as HTMLElement).matches('input');
  if ((ev.metaKey || ev.ctrlKey) && ev.key.toLowerCase() === 'k') {
    ev.preventDefault();
    $<HTMLInputElement>('q').focus();
    $<HTMLInputElement>('q').select();
    return;
  }
  if (ev.key === 'Escape') {
    if ($('addOv').classList.contains('show')) return closeAdd();
    if ($('saOv').classList.contains('show')) return closeStopAll();
    $<HTMLInputElement>('q').value = '';
    $<HTMLInputElement>('q').blur();
    renderList();
    return;
  }
  if (document.querySelector('.ov.show')) return; // dialogs own the keyboard
  if (inInput && !['ArrowDown', 'ArrowUp', 'Enter'].includes(ev.key)) return;
  const keys = [...document.querySelectorAll<HTMLElement>('[data-row]')].map((e) => e.dataset.row!);
  const i = selected ? keys.indexOf(selected) : -1;
  if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
    ev.preventDefault();
    const n = ev.key === 'ArrowDown' ? Math.min(keys.length - 1, i + 1) : Math.max(0, i - 1);
    if (keys[n]) select(keys[n]);
  } else if (ev.key === ' ' && selected && !inInput) {
    ev.preventDefault();
    toggle(selected);
  } else if (ev.key === 'Enter' && selected) {
    ev.preventDefault();
    if (inInput) $<HTMLInputElement>('q').blur();
    const r = snap.entries.find((e) => e.id === selected);
    const o = snap.ports.find((x) => `port:${x.port}` === selected);
    const url = r?.url ?? (o ? `http://localhost:${o.port}` : null);
    if (url && (!r || r.status === 'running' || !r.canStart)) open(url);
  }
});

$('q').addEventListener('input', () => renderList());
$('showAll').addEventListener('change', () => {
  showAll = $<HTMLInputElement>('showAll').checked;
  store.set('hub.showAll', showAll ? '1' : '0');
  render();
});
$('rescan').addEventListener('click', async () => {
  await refresh(true);
  toast(view === LOCAL ? `${snap.ports.length} ports open, ${snap.unhealthy} unhealthy` : `Scanned ${snap.root}: ${snap.entries.length} items`);
});

// ---- share on the LAN ---------------------------------------------------------------------------------------
async function copyLink(url: string, what: string) {
  try { await navigator.clipboard.writeText(url); toast(`${what} — link copied: ${url}`); } catch { toast(`${what}: ${url}`); }
}

async function share(port: number) {
  try {
    const r = await call<{ url: string; relay: boolean }>('share', { port });
    await refresh();
    copyLink(r.url, r.relay ? `:${port} shared on the local network` : `:${port} is reachable on the local network`);
  } catch (e: any) {
    toast(e.message, true);
  }
}

async function unshare(port: number) {
  const key = `share:${port}`;
  const p = snap.ports.find((x) => x.port === port);
  if (armed !== key) {
    armed = key;
    if (p?.lanUrl) copyLink(p.lanUrl, `:${port} on the LAN`);
    renderList();
    setTimeout(() => { if (armed === key) { armed = null; renderList(); } }, 3000);
    return;
  }
  armed = null;
  await call('unshare', { port }).catch((e) => toast(e.message, true));
  toast(`:${port} is back to this Mac only`);
  refresh();
}

async function addRoot() {
  try {
    const r = await call<{ ok: boolean; root?: string }>('addRoot', {});
    if (!r.ok) return;
    setView('all');
    await refresh(true);
    toast(`Added ${r.root} — ${snap.entries.length} items in all folders`);
  } catch (e: any) {
    toast(e.message, true);
  }
}

async function removeRoot(path: string) {
  const key = 'root:' + path;
  if (armed !== key) {
    armed = key;
    renderSide();
    setTimeout(() => { if (armed === key) { armed = null; renderSide(); } }, 3000);
    return;
  }
  armed = null;
  try {
    await call('removeRoot', { path });
    await refresh(true);
    toast(`${path} removed from the list — nothing was deleted`);
  } catch (e: any) {
    toast(e.message, true);
  }
}

// ---- stop all ---------------------------------------------------------------------------------------------
// In a browser the page itself lives on a Server Hub row (vite + bridge); stopping those would kill this window.
const selfPorts = isTauri ? [] : [Number(location.port), 4410];
const isSelf = (r: Row) => r.ports.some((p) => selfPorts.includes(p)) || (r.port != null && selfPorts.includes(r.port));
const stopTargets = () => snap.entries.filter((r) => isOn(r) && !isSelf(r));

function renderStopAll() {
  const rows = stopTargets();
  const out = $<HTMLInputElement>('saOut').checked ? snap.outside : [];
  const n = rows.length + out.length;
  $('saT').textContent = n ? `Stop ${n} server${n > 1 ? 's' : ''}?` : 'Nothing to stop';
  const line = (dot: string, name: string, ports: number[], by: string) =>
    `<div class="sa-row"><span class="dot ${dot}"></span><span class="mono">${esc(name)}</span><span class="mono faint">${ports.map((p) => ':' + p).join(' ')}</span><span class="faint">${esc(by)}</span></div>`;
  $('saList').innerHTML =
    rows.map((r) => line('running', r.name, r.ports.length ? r.ports : r.port ? [r.port] : [], r.own ? 'Server Hub' : r.source ? `by ${r.source}` : '')).join('') +
    out.map((o) => line('outside', o.name, o.ports, `by ${o.source}`)).join('') ||
    '<div class="sa-row"><span></span><span class="faint">No project server is running.</span></div>';
  $('saOutWrap').style.display = snap.outside.length ? '' : 'none';
  $('saOutT').textContent = `Also stop ${snap.outside.length} running outside (not from a project row)`;
  const yes = $<HTMLButtonElement>('saYes');
  yes.textContent = n ? `Stop ${n}` : 'Stop';
  yes.disabled = !n;
}

function openStopAll() {
  $<HTMLInputElement>('saOut').checked = false;
  renderStopAll();
  $('saOv').classList.add('show');
  $('saNo').focus();
}
const closeStopAll = () => $('saOv').classList.remove('show');
$('stopAll').addEventListener('click', openStopAll);
$('saNo').addEventListener('click', closeStopAll);
$('saOut').addEventListener('change', renderStopAll);
$('saYes').addEventListener('click', async () => {
  const rows = stopTargets();
  const out = $<HTMLInputElement>('saOut').checked ? snap.outside : [];
  closeStopAll();
  const res = await Promise.allSettled([
    ...rows.map((r) => call('stop', { id: r.id })),
    ...out.map((o) => call('kill', { pid: o.pid })),
  ]);
  const bad = res.filter((x) => x.status === 'rejected').length;
  toast(bad ? `Stopping ${res.length - bad}, ${bad} failed` : `Stopping ${res.length} server${res.length > 1 ? 's' : ''}…`, bad > 0);
  setTimeout(refresh, 500);
  setTimeout(refresh, 3500);
});

// ---- add by hand ----------------------------------------------------------------------------------------------
function closeAdd() { $('addOv').classList.remove('show'); }
$('addBtn').addEventListener('click', () => { $('aErr').textContent = ''; $('addOv').classList.add('show'); $('aName').focus(); });
$('aNo').addEventListener('click', closeAdd);
$('addForm').addEventListener('submit', async (ev) => {
  ev.preventDefault();
  const v = (id: string) => $<HTMLInputElement>(id).value.trim();
  try {
    await call('add', { name: v('aName'), cwd: v('aCwd'), command: v('aCmd'), port: v('aPort') });
    closeAdd();
    (ev.target as HTMLFormElement).reset();
    setView('g:Added by hand');
    await refresh(true);
  } catch (e: any) {
    $('aErr').textContent = e.message;
  }
});

refresh(true);
setInterval(() => { if (!document.hidden) refresh(); }, 2500);
setInterval(() => {
  const r = snap.entries.find((e) => e.id === selected);
  if (r && (r.own || r.status === 'starting')) loadLog();
}, 1000);
loadLog();
