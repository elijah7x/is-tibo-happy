// GitHub Actions 定时任务：抓 codex-reset.com → 归一化信封写进 public/state.json。
// 主源挂了退备用源 codex-resets.com；信封 source_url 记录实际来源。
// 源站永远只看到这一个访问者，客户端走 GitHub raw / jsDelivr CDN。
// 用法：在仓库根目录 node tools/mirror-fetch.mjs
import { writeFileSync, mkdirSync } from 'node:fs';
import { mirrorEnvelope, SOURCES } from '../src/net.mjs';

const UA = 'is-tibo-happy-mirror/0.1 (+https://github.com/elijah7x/is-tibo-happy)';

async function getJson(url) {
  const r = await fetch(url, {
    signal: AbortSignal.timeout(20000),
    headers: { 'User-Agent': UA, 'Accept': 'application/json' },
  });
  if (!r.ok) throw new Error(`${r.status} ${url.split('/')[2]}`);
  return r.json();
}

let upstream, src;
try {
  upstream = await getJson(SOURCES.direct);
  src = SOURCES.direct;
} catch (e) {
  console.error(`direct failed: ${e.message}; trying backup`);
  try {
    upstream = await getJson(SOURCES.backup);
    src = SOURCES.backup;
  } catch (e2) {
    console.error(`backup failed: ${e2.message}`);
    process.exit(1);   // Actions 标红但不提交，上一份 state.json 保留
  }
}

mkdirSync('public', { recursive: true });
writeFileSync('public/state.json', JSON.stringify(mirrorEnvelope(upstream, src), null, 2) + '\n');
console.log(`state.json updated via ${src}:`, upstream.updated_at || 'no updated_at');
