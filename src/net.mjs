// 拉取链：betteropc.com 产品页是第一信源（HTML 解析→归一化 forecast 形状）；
// 其下是 GitHub Actions 镜像（raw → 境内 jsDelivr CDN，每 20min 归一化），
// 都失败/过期再退 codex-reset.com → codex-resets.com 两个 JSON 源。每一环都有界超时。
// 返回 { forecast, via }；forecast 一律是对象（betteropc 页面在 fetch 时就地归一化，
// 不往缓存塞 HTML），交给 derive() 按形状分派。
import { parseBetteropc } from './state.mjs';

const REPO = 'elijah7x/is-tibo-happy';
export const SOURCES = {
  primary: 'https://betteropc.com/ai-products/reset-signals/codex',
  mirrors: [
    `https://raw.githubusercontent.com/${REPO}/main/public/state.json`,
    `https://cdn.jsdelivr.net/gh/${REPO}@main/public/state.json`,
  ],
  direct: 'https://codex-reset.com/api/forecast',
  backup: 'https://codex-resets.com/api/resets',
};
const MIRROR_MAX_AGE_MS = 6 * 3600e3;   // 镜像超过 6h 视为过期，降级直连

async function getJson(url, ua, timeoutMs) {
  const r = await fetch(url, {
    signal: AbortSignal.timeout(timeoutMs),
    headers: { 'User-Agent': ua, 'Accept': 'application/json' },
  });
  if (!r.ok) throw new Error(`${r.status} ${url.split('/')[2]}`);
  return r.json();
}

async function getText(url, ua, timeoutMs) {
  const r = await fetch(url, {
    signal: AbortSignal.timeout(timeoutMs),
    headers: { 'User-Agent': ua, 'Accept': 'text/html,*/*;q=0.8' },
  });
  if (!r.ok) throw new Error(`${r.status} ${url.split('/')[2]}`);
  return r.text();
}

const errMsg = e => e?.cause?.message || e?.message || String(e);   // fetch 的真实原因常在 e.cause

export async function fetchForecast(ua) {
  const errors = [];
  try {
    const bp = parseBetteropc(await getText(SOURCES.primary, ua, 12000));
    if (bp) return { forecast: bp, via: 'primary' };
    errors.push('betteropc: unrecognized page');
  } catch (e) { errors.push(errMsg(e)); }
  for (const url of SOURCES.mirrors) {
    try {
      const env = await getJson(url, ua, 8000);
      const fresh = env?.schema === 1 && env.upstream && typeof env.upstream === 'object' &&
        !Array.isArray(env.upstream) &&
        Number.isFinite(Date.parse(env.fetched_at)) &&
        Date.now() - Date.parse(env.fetched_at) < MIRROR_MAX_AGE_MS;
      if (fresh) return { forecast: env.upstream, via: 'mirror' };
      errors.push('mirror stale/invalid');
    } catch (e) { errors.push(errMsg(e)); }
  }
  try {
    return { forecast: await getJson(SOURCES.direct, ua, 15000), via: 'direct' };
  } catch (e) { errors.push(errMsg(e)); }
  try {
    return { forecast: await getJson(SOURCES.backup, ua, 15000), via: 'backup' };
  } catch (e) {
    errors.push(errMsg(e));
    throw new Error('all sources failed: ' + errors.join('; '));
  }
}

// 镜像侧（GH Actions）写出的信封格式——工具与客户端共用同一 schema。
export function mirrorEnvelope(upstream, sourceUrl = SOURCES.direct) {
  return {
    schema: 1,
    fetched_at: new Date().toISOString(),
    source_url: sourceUrl,
    upstream,
  };
}
