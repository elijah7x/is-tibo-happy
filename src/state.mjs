// 状态推导：上游 JSON → { kind, detail }
// kind: happy(有预告/暗示或近期重置) | unhappy(>3天没重置/无预告) | offline(连续≥12h拿不到数据)
// 两种上游形状：
//   forecast: codex-reset.com/api/forecast（commitment/tease_signal/last_reset_at/time_window）
//   resets:   codex-resets.com/api/resets（scheduled/events[].announced_at）
// 状态词与颜色是产品规则（用户定的搞怪规则），显示层只管渲染。

const UNHAPPY_AFTER_DAYS = 3;

const DAY = 86400e3, HOUR = 3600e3;

const WEEKDAYS = { sunday: 0, monday: 1, tuesday: 2, wednesday: 3, thursday: 4, friday: 5, saturday: 6 };
const CN_DAY = '日一二三四五六';
const EN_DAY = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
const EN_DAY_SHORT = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];

// 从预告原文解析目标日 → 'today' | 'tomorrow' | 'weekend' | 0-6（星期几），语言无关
function parseDayKey(text) {
  const t = (text || '').toLowerCase();
  if (!t) return null;
  if (/\btoday|tonight|end of day|eod\b/.test(t)) return 'today';
  if (/\btomorrow\b/.test(t)) return 'tomorrow';
  const m = t.match(/\b(sunday|monday|tuesday|wednesday|thursday|friday|saturday)\b/);
  if (m) return WEEKDAYS[m[1]];
  if (/\bweekend\b/.test(t)) return 'weekend';
  return null;
}

const validHour = h => Number.isFinite(h) && h >= 0 && h <= 23;
const utcMidnight = ms => {
  const d = new Date(ms);
  return Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate());
};

// "coming in Tuesday" → 具体 UTC 窗口：推文发布日（Tibo 时区 PT 口径）起第一个周二，
// 套上惯常重置时段（默认 23:00→次日 02:00 UTC，源站 time_window 字段可覆盖）
// → {start, end} 毫秒；解析不出 → null
function resolveTarget(text, postAtMs, timeWindow) {
  const dk = parseDayKey(text);
  if (dk == null) return null;
  const sh = validHour(timeWindow?.start_hour) ? timeWindow.start_hour : 23;
  const eh = validHour(timeWindow?.end_hour) ? timeWindow.end_hour : 2;
  // 基准日 = 发布时刻在 America/Los_Angeles 的本地日期（PT 傍晚≠UTC 同日），DST-safe
  const pt = localYMDW(Number.isFinite(postAtMs) ? postAtMs : Date.now(), 'America/Los_Angeles');
  const base = pt ? Date.UTC(+pt.ymd.slice(0, 4), +pt.ymd.slice(5, 7) - 1, +pt.ymd.slice(8, 10))
                  : utcMidnight(Date.now());
  let startDay, endDay;
  if (dk === 'today') startDay = endDay = base;
  else if (dk === 'tomorrow') startDay = endDay = base + DAY;
  else if (dk === 'weekend') {
    startDay = base;
    while (new Date(startDay).getUTCDay() !== 6) startDay += DAY;
    endDay = startDay + DAY;              // 窗口尾算到周日
  } else {
    startDay = base;
    while (new Date(startDay).getUTCDay() !== dk) startDay += DAY;
    endDay = startDay;
  }
  const start = startDay + sh * HOUR;
  let end = endDay + eh * HOUR;
  if (end <= start) end += DAY;           // 跨午夜窗口（23→2）落到次日
  return { start, end };
}

function setTarget(detail, text, postAtMs, timeWindow) {
  const t = resolveTarget(text, postAtMs, timeWindow);
  if (t) {
    detail.targetStart = new Date(t.start).toISOString();
    detail.targetEnd = new Date(t.end).toISOString();
  }
}

// 窗口字段合法性：两端都可解析且 end > start 才采纳
function setWindow(detail, win) {
  const s = Date.parse(win?.start), e = Date.parse(win?.end);
  if (Number.isFinite(s) && Number.isFinite(e) && e > s) {
    detail.windowStart = win.start;
    detail.windowEnd = win.end;
  }
}

export function deriveForecast(apiJson, now = Date.now()) {
  if (!apiJson || typeof apiJson !== 'object' || Array.isArray(apiJson)) {
    return { kind: 'offline', detail: {} };
  }
  const detail = {};

  // 1) 明确承诺：有预告不管多远都 HAPPY（倒计时 6 天也是 HAPPY）
  const commit = apiJson.commitment || apiJson.official_signal;
  if (commit) {
    detail.confirmed = true;
    detail.scheduledISO = commit.scheduled_for || commit.at || commit.time || null;
    detail.teaseText = commit.text || commit.quote || commit.display_text || null;
    detail.tweetUrl = commit.url || commit.tweet_url || null;
    setWindow(detail, apiJson.teased_window || commit.window);
    setTarget(detail, detail.teaseText,
      Date.parse(commit.at || commit.posted_at || '') || now, apiJson.time_window);
    return { kind: 'happy', detail };
  }

  // 2) 暗示级信号：未过期才算数（防止陈旧暗示残留）。
  // expires_at 存在时以它为准；缺失时按发布起 72h 封顶（不再永久有效）
  const tease = apiJson.tease_signal;
  const teasePostAt = Date.parse(tease?.post?.at || '');
  const teaseExp = Date.parse(tease?.expires_at || '');
  const teaseLive = tease && typeof tease === 'object' && tease.post && typeof tease.post === 'object'
    && Number.isFinite(teasePostAt)
    && (Number.isFinite(teaseExp) ? teaseExp > now : now - teasePostAt < 72 * HOUR);
  if (teaseLive) {
    detail.teaseText = tease.post.quote || null;
    detail.tweetUrl = tease.post.url || null;
    detail.teaseTier = tease.tier || null;
    setWindow(detail, apiJson.teased_window);
    setTarget(detail, detail.teaseText, teasePostAt, apiJson.time_window);
    return { kind: 'happy', detail };
  }

  // 2b) 独立窗口预告：无承诺无暗示但窗口合法且未结束，仍算信号（spec 规则 2）
  {
    const ws = Date.parse(apiJson.teased_window?.start), we = Date.parse(apiJson.teased_window?.end);
    if (Number.isFinite(ws) && Number.isFinite(we) && we > ws && we > now) {
      detail.windowStart = apiJson.teased_window.start;
      detail.windowEnd = apiJson.teased_window.end;
      return { kind: 'happy', detail };
    }
  }

  // 3) 账本：近期重置过依然 HAPPY（≤3 天）；>3 天且无信号 → UNHAPPY
  const last = Date.parse(apiJson.last_reset_at || '');
  if (!Number.isFinite(last)) {
    // 数据在但推不出 → 两态口径下归 unhappy
    return { kind: 'unhappy', detail };
  }
  detail.lastResetISO = new Date(last).toISOString();
  detail.daysSince = Math.max(0, (now - last) / DAY);   // 时钟偏差：未来时间按"刚刚"算
  detail.prob48 = apiJson.probabilities?.rounded_48h ?? null;
  return detail.daysSince > UNHAPPY_AFTER_DAYS
    ? { kind: 'unhappy', detail }
    : { kind: 'happy', detail };
}

// codex-resets.com/api/resets：{scheduled:{scheduled_for,display_text,announced_at,...}, events:[{announced_at}]}
export function deriveState(apiJson, now = Date.now()) {
  if (!apiJson || typeof apiJson !== 'object' || !Array.isArray(apiJson.events)) {
    return { kind: 'offline', detail: {} };
  }
  const detail = {};

  const s = apiJson.scheduled;
  if (s && typeof s === 'object') {
    if (Number.isFinite(Date.parse(s.scheduled_for || ''))) {
      detail.confirmed = true;
      detail.scheduledISO = s.scheduled_for;
      detail.teaseText = s.display_text || s.text || null;
      detail.resetType = s.reset_type || null;
      detail.tweetUrl = s.tweet_url || null;
      return { kind: 'happy', detail };
    }
    // 只有文本预告：48h 内宣布的才算活信号，过期视为陈旧忽略
    const txt = s.display_text || s.text || null;
    const ann = Date.parse(s.announced_at || '');
    if (txt && Number.isFinite(ann) && now - ann <= 48 * HOUR) {
      detail.teaseText = txt;
      detail.teaseTier = 'T1';
      detail.tweetUrl = s.tweet_url || null;
      setTarget(detail, txt, ann, apiJson.time_window);
      return { kind: 'happy', detail };
    }
  } else if (s) {
    detail.teaseText = String(s);
    return { kind: 'happy', detail };
  }

  const times = apiJson.events
    .map(e => Date.parse(e?.announced_at || ''))
    .filter(t => Number.isFinite(t));
  if (!times.length) return { kind: 'unhappy', detail };

  const last = Math.max(...times);
  detail.lastResetISO = new Date(last).toISOString();
  detail.daysSince = Math.max(0, (now - last) / DAY);

  return detail.daysSince > UNHAPPY_AFTER_DAYS
    ? { kind: 'unhappy', detail }
    : { kind: 'happy', detail };
}

// 按上游形状分派：events 数组 → resets 形状；普通对象 → forecast；其余 → offline
export function derive(upstream, now = Date.now()) {
  if (upstream && typeof upstream === 'object' && !Array.isArray(upstream)) {
    return Array.isArray(upstream.events)
      ? deriveState(upstream, now)
      : deriveForecast(upstream, now);
  }
  return { kind: 'offline', detail: {} };
}

// 副行文案（双语：产出 {zh, en}，widget 按界面语言挑选）
// 星期/今天/明天一律在用户本地时区判定；目标时刻是 UTC 窗口起点。

const COPY = {
  zh: {
    soon: '即将重置',
    hours: h => `${h} 小时后重置`,
    days: d => `${d} 天后重置`,
    byDay: dow => `最晚周${CN_DAY[dow]}重置`,
    expect: { today: '预计今天重置', tomorrow: '预计明天重置', dow: d => `预计周${CN_DAY[d]}重置` },
    announced: '已预告重置', hinted: '有重置暗示',
    justNow: '刚刚重置', ago: d => `上次重置 ${d} 天前`,
    none: '暂无重置预告', offline: '数据不可用',
  },
  en: {
    soon: 'reset imminent',
    hours: h => `reset in ~${h}h`,
    days: d => `reset in ${d} day${d === 1 ? '' : 's'}`,
    byDay: dow => `reset by ${EN_DAY[dow]}`,
    expect: { today: 'reset expected today', tomorrow: 'reset expected tomorrow', dow: d => `reset expected ${EN_DAY[d]}` },
    announced: 'reset announced', hinted: 'reset hinted',
    justNow: 'just reset', ago: d => `last reset ${d}d ago`,
    none: 'no reset news', offline: 'data unavailable',
  },
};

const dtfCache = new Map();
// 某时刻在 tz 下的本地日期与星期 → {ymd:'2026-09-22', dow:2}；tz 无效 → null
function localYMDW(ms, tz) {
  try {
    const key = tz || '';
    let dtf = dtfCache.get(key);
    if (!dtf) {
      dtf = new Intl.DateTimeFormat('en-US', {
        timeZone: tz, weekday: 'short', year: 'numeric', month: '2-digit', day: '2-digit',
      });
      dtfCache.set(key, dtf);
    }
    const p = {};
    for (const part of dtf.formatToParts(ms)) p[part.type] = part.value;
    return { ymd: `${p.year}-${p.month}-${p.day}`, dow: EN_DAY_SHORT.indexOf(p.weekday) };
  } catch { return null; }
}

function subLineIn(lang, { kind, detail }, now, tz) {
  const C = COPY[lang];
  const days = detail.daysSince != null ? Math.floor(detail.daysSince) : null;
  switch (kind) {
    case 'happy': {
      // 精确时间 → 倒计时（>48h 按天，1–48h 按小时，<1h 快到了）
      const t = detail.scheduledISO ? Date.parse(detail.scheduledISO) : NaN;
      if (Number.isFinite(t)) {
        if (t > now) {
          const h = (t - now) / HOUR;
          if (h < 1) return C.soon;
          if (h < 48) return C.hours(Math.round(h));
          return C.days(Math.round(h / 24));
        }
        // 刚过点算"即将"（重置传播需要时间），过太久回到"已预告"
        return now - t < 6 * HOUR ? C.soon : C.announced;
      }
      // 窗口制 → 窗口最晚边的本地星期（hedge 口径，学 codex-reset.com）
      if (detail.windowEnd) {
        const w = localYMDW(Date.parse(detail.windowEnd), tz);
        if (w && w.dow >= 0) return C.byDay(w.dow);
      }
      // 暗示/承诺：推文日子（PT 口径）+ UTC 窗口起点 → 本地星期/今天/明天
      const ts = detail.targetStart ? Date.parse(detail.targetStart) : NaN;
      if (Number.isFinite(ts)) {
        const te = detail.targetEnd ? Date.parse(detail.targetEnd) : NaN;
        // 已进入预告窗口 → 即将重置
        if (Number.isFinite(te) && ts <= now && now < te) return C.soon;
        // 窗口已过而暗示仍在 → 不做星期声明
        if (Number.isFinite(te) && te <= now) return C.hinted;
        const tgt = localYMDW(ts, tz), cur = localYMDW(now, tz), nxt = localYMDW(now + DAY, tz);
        if (tgt && cur && nxt) {
          // 相对词按用户本地日期说：目标窗口落在本地今天/明天就直说，其余报星期
          if (tgt.ymd === cur.ymd) return C.expect.today;
          if (tgt.ymd === nxt.ymd) return C.expect.tomorrow;
          if (tgt.dow >= 0) return C.expect.dow(tgt.dow);
        }
      }
      if (detail.confirmed) return C.announced;
      if (detail.teaseTier) return C.hinted;
      // 近期重置过（无预告的 happy）
      if (detail.daysSince != null)
        return detail.daysSince < 1 ? C.justNow : C.ago(days);
      return C.announced;
    }
    case 'unhappy': return C.none;   // 不数天数：避免与个人订阅自动重置周期混淆
    default: return C.offline;
  }
}

export function subLine(s, now = Date.now(), { tz } = {}) {
  return { zh: subLineIn('zh', s, now, tz), en: subLineIn('en', s, now, tz) };
}

// 守护进程展示决策：新数据 > 12h 内缓存 > OFFLINE。cache = { state, at(ms) } | null
export const CACHE_MAX_AGE_MS = 12 * 3600e3;
const SIGNAL_FIELDS = ['scheduledISO', 'windowEnd', 'targetStart', 'teaseTier', 'confirmed'];
export function resolveDisplay(fresh, cache, now = Date.now(), { tz } = {}) {
  if (fresh && fresh.kind !== 'offline') return { state: fresh, cache: { state: fresh, at: now } };
  const ok = cache && (cache.state?.kind === 'happy' || cache.state?.kind === 'unhappy')
    && cache.state.detail && typeof cache.state.detail === 'object'
    && Number.isFinite(cache.at) && now - cache.at < CACHE_MAX_AGE_MS && now - cache.at > -HOUR;
  if (!ok) {
    return { state: { kind: 'offline', detail: { sub: { zh: COPY.zh.offline, en: COPY.en.offline } } }, cache };
  }
  // 缓存命中：有可推导字段时按 now 重算文案与账龄（跨午夜"预计今天"不能原样回放）；
  // 无推导字段（只剩写死的 sub）时原样返回
  const s = cache.state;
  const last = Date.parse(s.detail.lastResetISO || '');
  const hasSignal = SIGNAL_FIELDS.some(f => s.detail[f]);
  if (!hasSignal && !Number.isFinite(last)) return { state: s, cache };
  const detail = { ...s.detail };
  if (Number.isFinite(last)) detail.daysSince = Math.max(0, (now - last) / DAY);
  const kind = hasSignal ? s.kind
    : (detail.daysSince != null && detail.daysSince <= UNHAPPY_AFTER_DAYS ? 'happy' : 'unhappy');
  const reworded = { kind, detail: { ...detail, sub: subLine({ kind, detail }, now, { tz }) } };
  return { state: reworded, cache };
}
