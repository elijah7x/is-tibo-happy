// GitHub Actions 定时任务：抓 codex-reset.com → 归一化信封写进 public/state.json。
// 源站按 IP/ASN 拦截数据中心（Actions runner 直连 403，换 UA 无效），
// 所以顺序是：直连 → 备用直连 → r.jina.ai 中继（runner 上可达）→ 中继备用。
// 源站永远只看到定时任务这一个访问者，客户端走 GitHub raw / jsDelivr CDN。
// 用法：在仓库根目录 node tools/mirror-fetch.mjs
import { writeFileSync, mkdirSync } from 'node:fs';
import { mirrorEnvelope, SOURCES } from '../src/net.mjs';

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
for (const [url, fn] of [
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
