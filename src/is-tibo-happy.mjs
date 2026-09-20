// is-tibo-happy 主控：启动宿主 → CDP 注入 widget → 抓数据推状态 → 断线自愈。
// 用法：node src/is-tibo-happy.mjs [--once] [--no-quit]
//   --once    注入+推一次状态后退出
//   --no-quit App 已运行但没开调试端口时，不自动重启它，直接报错退出
// 资源纪律：只在 App 活着时工作；用户退出 App 后原地等待（30s 轮询），不主动拉起。
import { listTargets, getVersion, connect, evalJs } from './cdp.mjs';
import { derive, subLine, resolveDisplay } from './state.mjs';
import { fetchForecast } from './net.mjs';
import { readFileSync, writeFileSync, unlinkSync, existsSync, statSync, truncateSync } from 'node:fs';
import { spawn, execSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import os from 'node:os';

const PORT = 9333;
// 宿主可能装在 /Applications 或用户级 ~/Applications
const APP = [`/Applications/ChatGPT.app`, `${os.homedir()}/Applications/ChatGPT.app`]
  .find(p => existsSync(p)) || null;
const EXE = APP ? `${APP}/Contents/MacOS/ChatGPT` : null;
const UA = 'is-tibo-happy/0.1 (+https://github.com/elijah7x/is-tibo-happy)'; // 自报家门：让源站能认出、限流、联系我们
const POLL_MS = 15 * 60 * 1000;   // 上游数据轮询
const RECONNECT_MS = 3000;        // CDP 断开后重试间隔
const APP_POLL_MS = 30 * 1000;    // App 不在时的等待轮询
const RELAUNCH_CAP = 3;           // 每小时最多重启 App 次数（防打架循环）
const RELAUNCH_WINDOW_MS = 3600e3;
const ONCE = process.argv.includes('--once');
const NO_QUIT = process.argv.includes('--no-quit');
const FORCE_LAUNCH = process.argv.includes('--launch');   // 手动授权：App 没在跑也拉起
const PIDFILE = fileURLToPath(new URL('../is-tibo-happy.pid', import.meta.url));
const LOGFILE = fileURLToPath(new URL('../daemon.log', import.meta.url));
const CACHE_FILE = fileURLToPath(new URL('../state-cache.json', import.meta.url));
// 安装器落的一次性标记：只在装后第一次运行时允许拉起 App（装完即生效），
// 此后进程重启/用户登录都只是"等待 App 回来"，不主动拉起。
const FIRST_FLAG = fileURLToPath(new URL('../.first-run', import.meta.url));
const WIDGET_SRC = readFileSync(new URL('./widget.js', import.meta.url), 'utf8');
const AVATAR_PATH = fileURLToPath(new URL('../assets/avatar/avatar.json', import.meta.url));
const AVATAR = (() => { try { return JSON.parse(readFileSync(AVATAR_PATH, 'utf8')); } catch { return null; } })();
const log = (...a) => console.log('[is-tibo-happy]', ...a);
const sleep = ms => new Promise(r => setTimeout(r, ms));

function acquireLock() {
  if (existsSync(PIDFILE)) {
    const pid = +readFileSync(PIDFILE, 'utf8').trim();
    if (pid) {
      // pid 可能被无关进程复用 → 校验进程身份确实是本守护进程
      let ours = false;
      try { ours = /node\S*\s+.*is-tibo-happy\.mjs(\s|$)/.test(execSync(`ps -p ${pid} -o args=`).toString()); } catch {}
      if (ours) { console.error(`[is-tibo-happy] already running (pid ${pid})`); process.exit(1); }
    }
  }
  writeFileSync(PIDFILE, String(process.pid));
  process.on('exit', () => { try { unlinkSync(PIDFILE); } catch {} });
}

const portUp = () => getVersion(PORT).then(() => true).catch(() => false);
const appRunning = () => {
  try { return execSync(`pgrep -f "${EXE}"`).toString().trim().length > 0; }
  catch { return false; }
};

async function spawnApp() {
  log(`launching ${APP} with debug port ${PORT}`);
  spawn(EXE, [`--remote-debugging-port=${PORT}`], { detached: true, stdio: 'ignore' })
    .on('error', e => log('spawn failed:', e.message)).unref();
  for (let i = 0; i < 45; i++) {
    await sleep(1000);
    if (await portUp()) return true;
  }
  return false;
}

// 近一小时内主动重启 App 的次数；两类拉起（重启带端口 / first-run 冷启）共用同一 cap
const relaunches = [];
let capLogged = false;   // cap 封顶日志只打一次，窗口滑出后重置
function relaunchAllowed() {
  const now = Date.now();
  while (relaunches.length && now - relaunches[0] > RELAUNCH_WINDOW_MS) relaunches.shift();
  if (relaunches.length < RELAUNCH_CAP) capLogged = false;
  if (relaunches.length >= RELAUNCH_CAP) {
    if (!capLogged) { log('relaunch cap reached; waiting for app to come back on its own'); capLogged = true; }
    return false;
  }
  relaunches.push(now);
  return true;
}

// 返回 true = 调试端口可用。launchIfAbsent=false 时：App 没在跑就原地等待（不拽起来）。
async function ensureApp({ launchIfAbsent = false } = {}) {
  if (await portUp()) return true;
  if (appRunning()) {
    // 在跑但没开调试端口 → 需要重启带端口；限速防反复打架
    if (NO_QUIT) throw new Error('app is running without debug port; quit it or drop --no-quit');
    if (!relaunchAllowed()) return false;
    log('app running without debug port; restarting it once');
    try { execSync(`osascript -e 'quit app "ChatGPT"'`); } catch {}
    for (let i = 0; i < 15 && appRunning(); i++) await sleep(1000);
    return spawnApp();
  }
  if (!launchIfAbsent || !relaunchAllowed()) return false;
  return spawnApp();
}

async function waitMainTarget() {
  for (let i = 0; i < 60; i++) {
    try {
      const t = (await listTargets(PORT)).find(x => x.type === 'page'
        && x.url.startsWith('app://-/index.html') && !x.url.includes('initialRoute='));
      if (t) return t;
    } catch {}
    await sleep(1000);
  }
  throw new Error('main window target never appeared');
}

let lastFetchAt = 0;   // 上次成功拉到上游数据的时间（menu-open 刷新节流的基准）
let lastFetchOk = false;
let lastErrText = '';  // 同一错误文本只打一次日志
// 落盘缓存：重启后仍可沿用最近一次好状态（{state, at}），12h 内有效
let cache = null;
try { cache = JSON.parse(readFileSync(CACHE_FILE, 'utf8')); } catch {}

async function fetchState() {
  let fresh = null;
  try {
    const { forecast, via } = await fetchForecast(UA);
    lastFetchAt = Date.now();
    fresh = derive(forecast);
    fresh.detail.sub = subLine(fresh);
    fresh.detail.via = via;
    lastFetchOk = true;
  } catch (e) {
    lastFetchOk = false;
    const t = String(e?.message || e);
    if (t !== lastErrText) { log('fetch failed:', t); lastErrText = t; }
  }
  const r = resolveDisplay(fresh, cache);
  if (r.cache !== cache) {
    cache = r.cache;
    try { writeFileSync(CACHE_FILE, JSON.stringify(cache)); } catch {}
  }
  return r.state;
}

async function main() {
  // 日志防膨胀：>1MB 截断重开
  try { if (existsSync(LOGFILE) && statSync(LOGFILE).size > 1024 * 1024) truncateSync(LOGFILE, 0); } catch {}
  if (!APP) {
    // 宿主没装：睡了再退，避免 launchd KeepAlive 把它拉成 tight loop
    log('ChatGPT.app not found in /Applications or ~/Applications; exiting');
    await sleep(60000);
    process.exit(0);
  }
  acquireLock();

  let stopping = false;
  const shutdown = async () => {
    stopping = true;
    if (active) {
      try { await evalJs(active, `window.__ith && window.__ith.destroy()`); } catch {}
      active.close();
    }
    process.exit(0);
  };
  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);

  let active = null;        // 当前活的 CDP 连接（断开期间为 null）
  let lastPushed = '';      // 推态去重（白名单键，剔除 daysSince 等易变字段）
  let injected = false;
  let pushing = false;      // push 防重入
  let retryTimer = null, retryDelay = 60e3;   // 拉取失败退避：1min 起 ×2，上限 15min
  process.on('exit', () => { if (retryTimer) clearTimeout(retryTimer); });
  const retry = () => {
    retryTimer = null;
    if (pushing) { retryTimer = setTimeout(retry, 1000); return; }   // evalJs 在途时稍后重试，防链断
    push().catch(() => {});
  };

  const push = async () => {
    if (!active || pushing) return;   // 断开期间不抓远端；进行中的 push 不叠加
    pushing = true;
    try {
      const s = await fetchState();
      if (lastFetchOk) {
        retryDelay = 60e3;
        if (retryTimer) { clearTimeout(retryTimer); retryTimer = null; }
      } else if (!retryTimer) {
        retryTimer = setTimeout(retry, retryDelay);
        retryDelay = Math.min(retryDelay * 2, 15 * 60e3);
      }
      const key = JSON.stringify({   // 内容指纹：只有用户可见信息变化才重推
        k: s.kind, sub: s.detail.sub,
        d: s.detail.daysSince == null ? null : Math.floor(s.detail.daysSince),
        s: s.detail.scheduledISO, w: s.detail.windowEnd, t: s.detail.targetStart,
      });
      if (key === lastPushed) return;
      lastPushed = key;
      log('state:', s.kind, `via=${s.detail.via || '-'}`, JSON.stringify(s.detail).slice(0, 120));
      try { await evalJs(active, `window.__ith && window.__ith.setState(${JSON.stringify(s)})`); }
      catch (e) { log('setState failed:', e.message); }
    } finally { pushing = false; }
  };

  const inject = async (cdp) => {
    await evalJs(cdp, WIDGET_SRC);
    injected = true;
    if (AVATAR) await evalJs(cdp, `window.__ith.setAvatar(${JSON.stringify(AVATAR)})`);
    // 缓存态先上屏，不等首轮 fetch（网络慢时菜单不至于空白）
    if (cache) {
      try { await evalJs(cdp, `window.__ith && window.__ith.setState(${JSON.stringify(cache.state)})`); } catch {}
    }
    lastPushed = '';          // 换页/重连后强制重推一次
    await push();
    log('widget injected' + (AVATAR ? ' +avatar' : ''));
  };

  // 一轮会话：连上主窗口 → 注入 → 挂到断开为止
  const session = async () => {
    const target = await waitMainTarget();
    let cdp = null;
    try {
    cdp = await connect(target.webSocketDebuggerUrl);
    active = cdp;
    log('main window:', target.id);
    await cdp.send('Runtime.enable');
    await cdp.send('Page.enable');
    // widget 每次成功挂卡 → window.ithRefresh('') → 这里收到通知后节流刷新
    await cdp.send('Runtime.addBinding', { name: 'ithRefresh' });
    cdp.on('Runtime.bindingCalled', p => {
      if (p.name !== 'ithRefresh') return;
      const payload = p.payload || '';
      if (payload.startsWith('menu-unmatched:')) { log('widget:', payload); return; }
      if (Date.now() - lastFetchAt < 5 * 60 * 1000) return;   // 5min 节流
      log('refresh via menu');
      push().catch(e => log('menu refresh failed:', e.message));
    });
    await inject(cdp);
    cdp.on('Page.frameNavigated', (f) => {
      if (f.frame?.parentId) return;
      injected = false;
      log('navigated, re-injecting');
      inject(cdp).catch(e => log('re-inject failed:', e.message));
    });
    if (ONCE) {
      await sleep(500);
      try { await evalJs(cdp, `window.__ith && window.__ith.destroy()`); } catch {}
      return;
    }
    await new Promise(res => cdp.onClose(res));
    active = null;
    injected = false;
    log('cdp disconnected');
    } finally {
      try { cdp?.close(); } catch {}   // 中途抛错/--once 早退时别留孤儿 ws（已关闭则幂等）
      if (active === cdp) active = null;
    }
  };

  let polling = false;   // 防重入：上一轮没做完就跳过
  const poll = setInterval(async () => {
    if (polling) return;
    polling = true;
    try {
      await push();
      if (active && !injected) await inject(active).catch(() => {});
    } finally { polling = false; }
  }, POLL_MS);
  process.on('exit', () => clearInterval(poll));

  // 主循环：仅"安装后首跑"（标记文件存在）或 --launch 时允许拉起 App；
  // 之后用户退出就只等不拉。首次成功会话后消费掉标记。
  while (!stopping) {
    const mayLaunch = FORCE_LAUNCH || existsSync(FIRST_FLAG);
    let up = false;
    try { up = await ensureApp({ launchIfAbsent: mayLaunch }); }
    catch (e) { log('ensureApp:', e.message); }
    if (!up) { if (ONCE) break; await sleep(APP_POLL_MS); continue; }
    try { unlinkSync(FIRST_FLAG); } catch {}   // 一次性授权已消费（不等 session，防残留循环拉起）
    try {
      await session();
    }
    catch (e) { log('session failed:', e.message); }
    if (ONCE || stopping) break;
    await sleep(RECONNECT_MS);
  }

  process.exit(0);
}

main().catch(e => { console.error('[is-tibo-happy] FATAL', e); process.exit(1); });
