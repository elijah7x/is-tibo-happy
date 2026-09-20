// 调试/验收：注入 widget → 逐状态强制渲染 + 真实数据态 → 截图。
// 用法：node src/inject-state.mjs [avatarJsonPath]
import { listTargets, connect, evalJs } from './cdp.mjs';
import { deriveForecast, subLine } from './state.mjs';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const PORT = 9333;
const API = 'https://codex-reset.com/api/forecast';
const UA = 'is-tibo-happy/0.1 (+https://github.com/elijah7x/is-tibo-happy)';
const OUT = fileURLToPath(new URL('../out/', import.meta.url));
mkdirSync(OUT, { recursive: true });
const WIDGET_SRC = readFileSync(new URL('./widget.js', import.meta.url), 'utf8');
const log = (...a) => console.log('[p0]', ...a);
const sleep = ms => new Promise(r => setTimeout(r, ms));

async function pressEscape(cdp) {
  for (const type of ['keyDown', 'keyUp'])
    await cdp.send('Input.dispatchKeyEvent', { type, key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
}

async function openMenu(cdp) {
  await evalJs(cdp, `
    var b = [...document.querySelectorAll('aside [aria-haspopup="menu"]')]
      .find(e => /prof|perfil|próf|προφ|проф|پروفا|نمایه|الملف|प्रोफ|ਪ੍ਰੋ|પ્રો|ಪ್ರೊ|പ്രൊ|ప్రొ|প্রোফাইল|சுயவிவர|პროფ|պրոֆ|โปรไฟล์|hồ sơ|wasifu|aqoonsiga|መገለጫ|ပရိုဖိုင်|プロフィール|프로필|个人资料|個人檔案|资料|檔案|账号|帳號|account|compte|cuenta|conta|アカウント|계정/i
        .test(e.getAttribute('aria-label') || ''));
    if (!b) throw new Error('profile button not found');
    var r = b.getBoundingClientRect();
    for (const t of ['pointerdown','pointerup','click'])
      b.dispatchEvent(new PointerEvent(t, {bubbles:true, cancelable:true, button:0, clientX:r.x+r.width/2, clientY:r.y+r.height/2}));
    'clicked'`);
  await sleep(700);
}

async function shot(cdp, name) {
  const s = await cdp.send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(OUT + name, Buffer.from(s.data, 'base64'));
  log('saved', name);
}

async function cardText(cdp) {
  return evalJs(cdp, `[...document.querySelectorAll('[data-ith]')].map(e=>e.textContent).join('|')`);
}

async function main() {
  const targets = await listTargets(PORT);
  const t = targets.find(x => x.type === 'page' && x.url === 'app://-/index.html');
  if (!t) throw new Error('main window target not found');
  const cdp = await connect(t.webSocketDebuggerUrl);
  await cdp.send('Runtime.enable');
  await cdp.send('Page.enable');

  await pressEscape(cdp);
  await sleep(300);
  await evalJs(cdp, WIDGET_SRC);
  log('widget injected');

  // 若传入 avatar.json 路径则先 setAvatar
  if (process.argv[2]) {
    const a = JSON.parse(readFileSync(process.argv[2], 'utf8'));
    log('setAvatar:', await evalJs(cdp, `window.__ith.setAvatar(${JSON.stringify(a)}); 'ok'`));
  }

  for (const kind of ['happy', 'unhappy', 'offline']) {
    await evalJs(cdp, `window.__ith.force('${kind}'); 'ok'`);
    await openMenu(cdp);
    log(kind, 'card:', await cardText(cdp));
    await shot(cdp, `p0-${kind}.png`);
    await pressEscape(cdp);
    await sleep(400);
  }

  // 真实数据态
  try {
    const r = await fetch(API, { signal: AbortSignal.timeout(15000), headers: { 'User-Agent': UA } });
    const s = deriveForecast(await r.json());
    s.detail.sub = subLine(s);
    log('live derive:', s.kind, JSON.stringify(s.detail));
    await evalJs(cdp, `window.__ith.setState(${JSON.stringify(s)}); 'ok'`);
  } catch (e) {
    log('live fetch failed:', String(e));
    await evalJs(cdp, `window.__ith.setState({kind:'offline',detail:{}}); 'ok'`);
  }
  await openMenu(cdp);
  log('live card:', await cardText(cdp));
  await shot(cdp, 'p0-live.png');
  await pressEscape(cdp);

  cdp.close();
}

main().catch(e => { console.error('[p0] FATAL', e); process.exit(1); });
