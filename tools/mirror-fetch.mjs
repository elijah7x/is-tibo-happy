// GitHub Actions 定时任务：抓第一信源 betteropc.com（HTML→归一化）→ 信封写进 public/state.json。
// betteropc 失败时回退 JSON 链：codex-reset.com 直连 → 备用直连 → r.jina.ai 中继 → 中继备用
// （codex-reset 按 IP/ASN 拦数据中心，runner 上靠中继可达）。
// 源站永远只看到定时任务这一个访问者，客户端走 GitHub raw / jsDelivr CDN。
// 用法：在仓库根目录 node tools/mirror-fetch.mjs
import { writeFileSync, mkdirSync } from 'node:fs';
import { mirrorEnvelope, SOURCES } from '../src/net.mjs';
import { parseBetteropc } from '../src/state.mjs';

const UA = 'is-tibo-happy-mirror/0.1 (+https://github.com/elijah7x/is-tibo-happy)';
const RELAY = 'https://r.jina.ai/';

async function getJson(url) {
  const r = await fetch(url, {
    signal: AbortSignal.timeout(20000),
    headers: { 'User-Agent': UA, 'Accept': 'application/json' },
  });
  if (!r.ok) throw new Error(`${r.status} ${url.split('/')[2]}`);
  return r.json();
}

async function getText(url) {
  const r = await fetch(url, {
    signal: AbortSignal.timeout(20000),
    headers: { 'User-Agent': UA, 'Accept': 'text/html,*/*;q=0.8' },
  });
  if (!r.ok) throw new Error(`${r.status} ${url.split('/')[2]}`);
  return r.text();
}

// r.jina.ai 返回 "Title:/URL Source:/Markdown Content:" 封皮 + JSON 正文，按花括号切片
async function getJsonRelayed(url) {
  const r = await fetch(RELAY + url, {
    signal: AbortSignal.timeout(45000),
    headers: { 'User-Agent': UA },
  });
  if (!r.ok) throw new Error(`${r.status} relay`);
  const text = await r.text();
  const i = text.indexOf('{'), j = text.lastIndexOf('}');
  if (i < 0 || j <= i) throw new Error('relay: no json in body');
  return JSON.parse(text.slice(i, j + 1));
}

let upstream, src;
// 第一信源：betteropc 产品页（HTML → 归一化 forecast 形状）
try {
  const bp = parseBetteropc(await getText(SOURCES.primary));
  if (bp) { upstream = bp; src = SOURCES.primary; }
  else console.error('betteropc: page not recognized');
} catch (e) { console.error(`betteropc failed: ${e.message}`); }
// 回退：原 JSON 源链
if (!upstream) for (const [url, fn] of [
  [SOURCES.direct, getJson],
  [SOURCES.backup, getJson],
  [SOURCES.direct, getJsonRelayed],
  [SOURCES.backup, getJsonRelayed],
]) {
  try { upstream = await fn(url); src = url; break; }
  catch (e) { console.error(`${fn === getJsonRelayed ? 'relay ' : ''}${url.split('/')[2]} failed: ${e.message}`); }
}
if (!upstream) process.exit(1);   // Actions 标红但不提交，上一份 state.json 保留

mkdirSync('public', { recursive: true });
writeFileSync('public/state.json', JSON.stringify(mirrorEnvelope(upstream, src), null, 2) + '\n');
console.log(`state.json updated via ${src}:`, upstream.updated_at || 'no updated_at');
