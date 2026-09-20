// 拉取链：镜像优先（GitHub Actions 每 20min 归一化 public/state.json），
// 境内走 jsDelivr CDN，都失败/过期才直连源站，源站改版再退备用源。每一环都有界超时。
// 返回 { forecast, via }；forecast 是上游原始 JSON（镜像只做信封，
// upstream 可能是 forecast 或 backup 形状，交给 derive() 分派）。
const REPO = 'elijah7x/is-tibo-happy';
export const SOURCES = {
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

const errMsg = e => e?.cause?.message || e?.message || String(e);   // fetch 的真实原因常在 e.cause

export async function fetchForecast(ua) {
  const errors = [];
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
