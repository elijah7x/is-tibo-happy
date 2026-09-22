// 状态推导：上游 JSON → { kind, detail }（state.mjs 的 Rust 移植，语义逐行对齐）
// kind: happy(有预告/暗示或近期重置) | unhappy(>3天没重置/无预告) | offline(连续≥12h拿不到数据)
// 两种上游形状：
//   forecast: codex-reset.com/api/forecast（commitment/tease_signal/last_reset_at/time_window）
//   resets:   codex-resets.com/api/resets（scheduled/events[].announced_at）
// 状态词与颜色是产品规则，显示层只管渲染。
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::LazyLock;

pub const UNHAPPY_AFTER_DAYS: f64 = 3.0;
// 预告极少跳票只会迟到：目标窗口结束后再宽限 36h 才算信号失效；
// 信号发布距今 >7 天一律视为陈旧
pub const LATE_GRACE: i64 = 36 * 3600_000;
pub const SIG_MAX_AGE: i64 = 7 * 86400_000;
pub const DAY: i64 = 86400_000;
pub const HOUR: i64 = 3600_000;

const CN_DAY: &[char] = &['日', '一', '二', '三', '四', '五', '六'];
const EN_DAY: &[&str] = &["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

// 上游时间一律按 UTC 绝对时刻解析（JS Date.parse 对无 Z 后缀按本地时区，这里更严：
// 无偏移量也按 UTC——上游只会给 ISO/Z 格式，严格解析顺便挡垃圾输入）
fn parse_iso(s: &str) -> Option<i64> {
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.timestamp_millis());
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(n) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(n.and_utc().timestamp_millis());
        }
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return d.and_hms_opt(0, 0, 0).map(|n| n.and_utc().timestamp_millis());
    }
    None
}

// JS Date.parse(v)：只接受字符串值，其余（数字/对象/null）→ None
fn parse_ms(v: Option<&Value>) -> Option<i64> {
    v.and_then(|v| v.as_str()).and_then(parse_iso)
}

pub fn parse_iso_pub(s: &str) -> Option<i64> {
    parse_iso(s)
}
pub fn iso_pub(ms: i64) -> String {
    iso(ms)
}

fn iso(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|d| d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_default()
}

fn get<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.as_object().and_then(|o| o.get(k))
}

// ── 预告原文 → 目标日 ──
#[derive(Clone, Copy)]
enum DayKey {
    Today,
    Tomorrow,
    Weekend,
    Dow(u32), // 0=周日
}

static RE_TODAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\btoday|tonight|end of day|eod\b").unwrap());
static RE_TOMORROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\btomorrow\b").unwrap());
static RE_WEEKDAY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(sunday|monday|tuesday|wednesday|thursday|friday|saturday)\b").unwrap());
static RE_WEEKEND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bweekend\b").unwrap());

fn parse_day_key(text: &Value) -> Option<DayKey> {
    let t = text.as_str()?.to_lowercase();
    if t.is_empty() {
        return None;
    }
    if RE_TODAY.is_match(&t) {
        return Some(DayKey::Today);
    }
    if RE_TOMORROW.is_match(&t) {
        return Some(DayKey::Tomorrow);
    }
    if let Some(m) = RE_WEEKDAY.captures(&t) {
        let dow = EN_DAY.iter().position(|d| d.to_lowercase() == m[1]).unwrap() as u32;
        return Some(DayKey::Dow(dow));
    }
    if RE_WEEKEND.is_match(&t) {
        return Some(DayKey::Weekend);
    }
    None
}

fn valid_hour(v: Option<&Value>) -> Option<i64> {
    v.and_then(|v| v.as_f64()).filter(|h| *h >= 0.0 && *h <= 23.0).map(|h| h as i64)
}

fn utc_midnight(ms: i64) -> i64 {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|d| {
            Utc.with_ymd_and_hms(d.year(), d.month(), d.day(), 0, 0, 0)
                .single()
                .map(|m| m.timestamp_millis())
                .unwrap_or(ms)
        })
        .unwrap_or(ms)
}

fn utc_dow(ms: i64) -> u32 {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|d| d.weekday().num_days_from_sunday())
        .unwrap_or(0)
}

// 某时刻在 tz 下的本地日期与星期 → ("2026-09-22", 2)；tz 无效 → None
// tz=None 时跟 Intl 一样用系统本地时区
fn local_ymd_w(ms: i64, tz: Option<&str>) -> Option<(String, u32)> {
    let name = match tz {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => iana_time_zone::get_timezone().ok()?,
    };
    let zone: Tz = name.parse().ok()?;
    let dt = Utc.timestamp_millis_opt(ms).single()?.with_timezone(&zone);
    Some((dt.format("%Y-%m-%d").to_string(), dt.weekday().num_days_from_sunday()))
}

#[derive(Clone, Copy)]
struct Target {
    start: i64,
    end: i64,
}

// "coming in Tuesday" → 具体 UTC 窗口：推文发布日（Tibo 时区 PT 口径）起第一个周二，
// 套上惯常重置时段（默认 23:00→次日 02:00 UTC，源站 time_window 字段可覆盖）
fn resolve_target(text: &Value, post_at_ms: Option<i64>, time_window: Option<&Value>, now: i64) -> Option<Target> {
    let dk = parse_day_key(text)?;
    let sh = valid_hour(time_window.and_then(|w| get(w, "start_hour"))).unwrap_or(23);
    let eh = valid_hour(time_window.and_then(|w| get(w, "end_hour"))).unwrap_or(2);
    // 基准日 = 发布时刻在 America/Los_Angeles 的本地日期（PT 傍晚≠UTC 同日），DST-safe
    let base = local_ymd_w(post_at_ms.unwrap_or(now), Some("America/Los_Angeles"))
        .and_then(|(ymd, _)| {
            let y: i32 = ymd[0..4].parse().ok()?;
            let m: u32 = ymd[5..7].parse().ok()?;
            let d: u32 = ymd[8..10].parse().ok()?;
            Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).single().map(|t| t.timestamp_millis())
        })
        .unwrap_or_else(|| utc_midnight(now));
    let (mut start_day, end_day);
    match dk {
        DayKey::Today => {
            start_day = base;
            end_day = base;
        }
        DayKey::Tomorrow => {
            start_day = base + DAY;
            end_day = start_day;
        }
        DayKey::Weekend => {
            start_day = base;
            while utc_dow(start_day) != 6 {
                start_day += DAY;
            }
            end_day = start_day + DAY; // 窗口尾算到周日
        }
        DayKey::Dow(want) => {
            start_day = base;
            while utc_dow(start_day) != want {
                start_day += DAY;
            }
            end_day = start_day;
        }
    }
    let start = start_day + sh * HOUR;
    let mut end = end_day + eh * HOUR;
    if end <= start {
        end += DAY; // 跨午夜窗口（23→2）落到次日
    }
    Some(Target { start, end })
}

fn set_target(detail: &mut Map<String, Value>, text: &Value, post_at: Option<i64>, tw: Option<&Value>, now: i64) {
    if let Some(t) = resolve_target(text, post_at, tw, now) {
        detail.insert("targetStart".into(), json!(iso(t.start)));
        detail.insert("targetEnd".into(), json!(iso(t.end)));
    }
}

// 目标窗口结束 + 迟到宽限内仍算活信号（预告极少跳票，只会早来或迟到）
fn still_fresh(t: &Option<Target>, now: i64) -> bool {
    t.as_ref().map(|t| now < t.end + LATE_GRACE).unwrap_or(false)
}

// 窗口字段合法性：两端都可解析且 end > start 才采纳
fn set_window(detail: &mut Map<String, Value>, win: Option<&Value>) {
    let (Some(s), Some(e)) = (get(win.unwrap_or(&Value::Null), "start"), get(win.unwrap_or(&Value::Null), "end"))
    else {
        return;
    };
    let (Some(sm), Some(em)) = (parse_ms(Some(s)), parse_ms(Some(e))) else {
        return;
    };
    if em > sm {
        detail.insert("windowStart".into(), s.clone());
        detail.insert("windowEnd".into(), e.clone());
    }
}

fn str_field(v: Option<&Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| get(v.unwrap_or(&Value::Null), k).and_then(|x| x.as_str()).map(String::from))
}

pub fn derive_forecast(api: &Value, now: i64) -> Value {
    if !api.is_object() {
        return json!({"kind": "offline", "detail": {}});
    }
    let last = parse_ms(get(api, "last_reset_at"));
    // 信号发布时间早于最近一次重置 → 该预告已兑现，让位账本分支（"刚刚重置"）
    let fulfilled = |at: Option<i64>| matches!(at, Some(a) if last.is_some_and(|l| a < l));

    // 1) 明确承诺：有预告不管多远都 HAPPY（倒计时 6 天也是 HAPPY）
    let commit = get(api, "commitment").or_else(|| get(api, "official_signal"));
    if let Some(commit) = commit {
        let mut d = Map::new();
        d.insert("confirmed".into(), json!(true));
        if let Some(s) = str_field(Some(commit), &["scheduled_for", "time"]) {
            d.insert("scheduledISO".into(), json!(s));
        }
        if let Some(s) = str_field(Some(commit), &["text", "quote", "display_text"]) {
            d.insert("teaseText".into(), json!(s));
        }
        if let Some(s) = str_field(Some(commit), &["url", "tweet_url"]) {
            d.insert("tweetUrl".into(), json!(s));
        }
        set_window(&mut d, get(api, "teased_window").or_else(|| get(commit, "window")));
        let post_at = parse_ms(get(commit, "posted_at").or_else(|| get(commit, "announced_at")).or_else(|| get(commit, "at")));
        if post_at.is_some() {
            let txt = d.get("teaseText").cloned().unwrap_or(Value::Null);
            set_target(&mut d, &txt, post_at, get(api, "time_window"), now);
        }
        let sf = parse_ms(d.get("scheduledISO"));
        let we = parse_ms(d.get("windowEnd"));
        // 至少要有一个可解析的时间锚才算信号：scheduled_for / 活窗口 / 近 7 天发布；
        // 全部缺失 = 无从校验的"永生信号"（无锚文本每次推导还会重锚到下一个周二），不算数
        let anchored = sf.is_some()
            || we.is_some_and(|w| w + LATE_GRACE > now)
            || post_at.is_some_and(|p| now - p < SIG_MAX_AGE);
        let stale = sf.is_some_and(|s| s + LATE_GRACE <= now); // 过点超宽限 = 跳票
        if anchored && !stale && !fulfilled(post_at.or(sf)) {
            return json!({"kind": "happy", "detail": d});
        }
    }

    // 2) 暗示级信号：上游 expires_at 失效、72h 新鲜度、目标窗口+迟到宽限，任一存活即算数
    let tease = get(api, "tease_signal");
    let post = tease.and_then(|t| get(t, "post")).filter(|p| p.is_object());
    let tease_post_at = parse_ms(post.and_then(|p| get(p, "at")));
    if let (Some(t), Some(p), Some(tp)) = (tease, post, tease_post_at) {
        if now - tp < SIG_MAX_AGE && !fulfilled(Some(tp)) {
            let tease_exp = parse_ms(get(t, "expires_at"));
            let tt = resolve_target(get(p, "quote").unwrap_or(&Value::Null), Some(tp), get(api, "time_window"), now);
            if tease_exp.is_some_and(|e| e > now) || now - tp < 72 * HOUR || still_fresh(&tt, now) {
                let mut d = Map::new();
                if let Some(s) = get(p, "quote").and_then(|q| q.as_str()) {
                    d.insert("teaseText".into(), json!(s));
                }
                if let Some(s) = get(p, "url").and_then(|u| u.as_str()) {
                    d.insert("tweetUrl".into(), json!(s));
                }
                if let Some(s) = get(t, "tier").and_then(|x| x.as_str()) {
                    d.insert("teaseTier".into(), json!(s));
                }
                set_window(&mut d, get(api, "teased_window"));
                if let Some(tt) = tt {
                    d.insert("targetStart".into(), json!(iso(tt.start)));
                    d.insert("targetEnd".into(), json!(iso(tt.end)));
                }
                return json!({"kind": "happy", "detail": d});
            }
        }
    }

    // 2b) 弱信号兜底：上游撤了 tease_signal 但 latest_hint（近 7 天的暗示推文）仍在，
    // 且目标窗口未过迟到宽限 → 维持 hedge 级 HAPPY。quote 解不出目标则不算信号。
    let hint = get(api, "latest_hint").filter(|h| h.is_object());
    let hint_at = parse_ms(hint.and_then(|h| get(h, "at")));
    if let (Some(h), Some(ha)) = (hint, hint_at) {
        if now - ha < SIG_MAX_AGE && !fulfilled(Some(ha)) {
            let ht = resolve_target(get(h, "quote").unwrap_or(&Value::Null), Some(ha), get(api, "time_window"), now);
            if let (true, Some(ht)) = (still_fresh(&ht, now), ht) {
                let mut d = Map::new();
                if let Some(s) = get(h, "quote").and_then(|q| q.as_str()) {
                    d.insert("teaseText".into(), json!(s));
                }
                if let Some(s) = get(h, "url").and_then(|u| u.as_str()) {
                    d.insert("tweetUrl".into(), json!(s));
                }
                d.insert("teaseTier".into(), json!("hint"));
                d.insert("targetStart".into(), json!(iso(ht.start)));
                d.insert("targetEnd".into(), json!(iso(ht.end)));
                return json!({"kind": "happy", "detail": d});
            }
        }
    }

    // 2c) 独立窗口预告：无承诺无暗示但窗口合法且未过迟到宽限，仍算信号
    {
        let win = get(api, "teased_window");
        let ws = parse_ms(win.and_then(|w| get(w, "start")));
        let we = parse_ms(win.and_then(|w| get(w, "end")));
        if let (Some(s), Some(e)) = (ws, we) {
            if e > s && e + LATE_GRACE > now && !fulfilled(Some(s)) {
                return json!({"kind": "happy", "detail": {
                    "windowStart": win.and_then(|w| get(w, "start")).cloned().unwrap_or(Value::Null),
                    "windowEnd": win.and_then(|w| get(w, "end")).cloned().unwrap_or(Value::Null),
                }});
            }
        }
    }

    // 3) 账本：近期重置过依然 HAPPY（≤3 天）；>3 天且无信号 → UNHAPPY
    let Some(last) = last else {
        // 数据在但推不出 → 两态口径下归 unhappy
        return json!({"kind": "unhappy", "detail": {}});
    };
    let days_since = ((now - last) as f64 / DAY as f64).max(0.0); // 时钟偏差：未来时间按"刚刚"算
    let mut detail = json!({
        "lastResetISO": iso(last),
        "daysSince": days_since,
    });
    if let Some(p) = get(api, "probabilities").and_then(|p| get(p, "rounded_48h")) {
        detail["prob48"] = p.clone();
    } else {
        detail["prob48"] = Value::Null;
    }
    if days_since > UNHAPPY_AFTER_DAYS {
        json!({"kind": "unhappy", "detail": detail})
    } else {
        json!({"kind": "happy", "detail": detail})
    }
}

// codex-resets.com/api/resets：{scheduled:{scheduled_for,display_text,announced_at,...}, events:[{announced_at}]}
pub fn derive_state(api: &Value, now: i64) -> Value {
    let events = match get(api, "events").and_then(|e| e.as_array()) {
        Some(e) if api.is_object() => e,
        _ => return json!({"kind": "offline", "detail": {}}),
    };
    let mut last: Option<i64> = None;
    let mut last_type: Option<String> = None; // 最近一次事件的 reset_type（banked/regular）
    for e in events {
        if let Some(t) = parse_ms(get(e, "announced_at")) {
            if last.is_none_or(|l| t >= l) {
                last = Some(t);
                last_type = get(e, "reset_type").and_then(|r| r.as_str()).map(String::from);
            }
        }
    }
    // 信号发布时间早于最近事件 → 预告已兑现，让位账本
    let fulfilled = |at: Option<i64>| matches!(at, Some(a) if last.is_some_and(|l| a < l));

    match get(api, "scheduled") {
        Some(s) if s.is_object() => {
            if let Some(sf) = parse_ms(get(s, "scheduled_for")) {
                let ann = parse_ms(get(s, "announced_at"));
                // scheduled 时刻已过宽限或已被账本兑现 → 不再算信号
                if sf + LATE_GRACE > now && !fulfilled(ann.or(Some(sf))) {
                    let mut d = Map::new();
                    d.insert("confirmed".into(), json!(true));
                    d.insert("scheduledISO".into(), get(s, "scheduled_for").cloned().unwrap_or(Value::Null));
                    if let Some(t) = str_field(Some(s), &["display_text", "text"]) {
                        d.insert("teaseText".into(), json!(t));
                    }
                    if let Some(t) = str_field(Some(s), &["reset_type"]) {
                        d.insert("resetType".into(), json!(t));
                    }
                    if let Some(t) = str_field(Some(s), &["tweet_url"]) {
                        d.insert("tweetUrl".into(), json!(t));
                    }
                    return json!({"kind": "happy", "detail": d});
                }
            } else {
                // 只有文本预告：48h 内宣布或目标窗口未过迟到宽限才算活信号
                let txt = str_field(Some(s), &["display_text", "text"]).map(Value::String).unwrap_or(Value::Null);
                let ann = parse_ms(get(s, "announced_at"));
                let tt = ann.and_then(|a| resolve_target(&txt, Some(a), get(api, "time_window"), now));
                if let (Some(tt), Some(a)) = (tt, ann) {
                    if (now - a <= 48 * HOUR || still_fresh(&Some(tt), now)) && !fulfilled(Some(a)) {
                        return json!({"kind": "happy", "detail": {
                            "teaseText": txt,
                            "teaseTier": "T1",
                            "tweetUrl": get(s, "tweet_url").cloned().unwrap_or(Value::Null),
                            "targetStart": iso(tt.start),
                            "targetEnd": iso(tt.end),
                        }});
                    }
                }
            }
        }
        Some(s) => {
            // scheduled 是裸字符串：只有能解析成时刻且未过宽限才算信号，否则落账本
            let t = s.as_str().and_then(parse_iso);
            if let Some(t) = t {
                if t + LATE_GRACE > now && !fulfilled(Some(t)) {
                    return json!({"kind": "happy", "detail": {
                        "confirmed": true,
                        "scheduledISO": s.clone(),
                    }});
                }
            }
        }
        None => {}
    }

    if last.is_none() {
        return json!({"kind": "unhappy", "detail": {}});
    }
    let last = last.unwrap();
    let days_since = ((now - last) as f64 / DAY as f64).max(0.0);
    let mut detail = json!({
        "lastResetISO": iso(last),
        "daysSince": days_since,
    });
    if let Some(t) = last_type {
        detail["resetType"] = json!(t);
    }
    if days_since > UNHAPPY_AFTER_DAYS {
        json!({"kind": "unhappy", "detail": detail})
    } else {
        json!({"kind": "happy", "detail": detail})
    }
}

// 按上游形状分派：events 数组 → resets 形状；普通对象 → forecast；其余 → offline
pub fn derive(upstream: &Value, now: i64) -> Value {
    if upstream.is_object() {
        if get(upstream, "events").is_some_and(|e| e.is_array()) {
            derive_state(upstream, now)
        } else {
            derive_forecast(upstream, now)
        }
    } else {
        json!({"kind": "offline", "detail": {}})
    }
}

// ── 第一信源 betteropc.com：HTML 页面 → 归一化 forecast 形状 ──
// 无公开 JSON API，只读页面自带的机器可读字段（DOM data-* 与 RSC props 同源重复）：
//   .product-tracking-last-confirmed-reset[dateTime]   最近确认重置（UTC 绝对时刻）
//   .product-tracking-scheduled-reset 卡 + targetIso/publishedAtIso   下一次重置（作者给的明确时刻）
//   [data-reset-today-status/date]                     当日状态（北京口径，仅记录不驱动情绪）
// 归一化后走 deriveForecast，倒计时/迟到宽限/兑现让位/锚定规则全部复用。
fn bp_attr(name: &str) -> Regex {
    Regex::new(&format!(r#"{name}["'\\:= ]+([^"'\\,}}]+)"#)).unwrap()
}

fn bp_attr_get<'a>(name: &str, seg: &'a str) -> Option<&'a str> {
    bp_attr(name).captures(seg).and_then(|c| c.get(1)).map(|m| m.as_str())
}

// 时间字段必须长得像 ISO 时刻，否则按缺失处理——捕获到标记文本/":"/相邻值都算无效
static BP_ISO_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}").unwrap());
static BP_DATE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}").unwrap());
static BP_STATUS_BAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)cancel|missed|fail|executed|confirmed|done|landed|expired|取消|已执行|已完成|已重置|错过|跳票|失效|过期").unwrap()
});
static BP_X_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"x\.com/[A-Za-z0-9_]+/status/\d+").unwrap());

fn bp_iso(v: Option<&str>) -> Option<String> {
    v.filter(|s| BP_ISO_RE.is_match(s)).map(String::from)
}
fn bp_date(v: Option<&str>) -> Option<String> {
    v.filter(|s| BP_DATE_RE.is_match(s)).map(String::from)
}

pub fn parse_betteropc(html: &str) -> Option<Value> {
    // 守卫：必须是 betteropc 的 Codex 产品页。product-tracking-* 是结构标记，
    // data-product-id="codex" 才是产品身份——URL 改版重定向到别家产品时宁可判失败走降级链
    if !html.contains("product-tracking-") {
        return None;
    }
    if !bp_attr_get("data-product-id", html).is_some_and(|v| v.eq_ignore_ascii_case("codex")) {
        return None;
    }
    let mut out = json!({"source": "betteropc", "updated_at": bp_iso(bp_attr_get("data-product-updated-at", html))});
    if let Some(ts) = bp_attr_get("data-reset-today-status", html) {
        out["today"] = json!({"status": ts, "date": bp_date(bp_attr_get("data-reset-today-date", html))});
    }
    // 账本：dateTime 必须在"包含标记的那个 <time> 标签"内（DOM 字段顺序无关）；
    // 找不到标签（纯 RSC 形态）退化为标记两侧 300 字符内最近的一个——props 字段顺序
    // 一改就会把相邻元素的 dateTime 误抓进来
    let mut last_reset_at: Option<String> = None;
    if let Some(li) = html.find("product-tracking-last-confirmed-reset") {
        let open = html[..li].rfind("<time");
        let end = open.and_then(|o| html[o..].find('>').map(|e| o + e));
        if matches!((open, end), (Some(_), Some(e)) if e > li) {
            last_reset_at = bp_iso(bp_attr_get("dateTime", &html[open.unwrap()..end.unwrap() + 1]));
        } else {
            // 标记后首个 vs 标记前末个，取更近者
            let fwd = bp_attr("dateTime").find(&html[li..li + 300.min(html.len() - li)]);
            let bseg_end = li;
            let bseg_start = li.saturating_sub(300);
            let bseg = &html[bseg_start..bseg_end];
            let bwd = bp_attr("dateTime").find_iter(bseg).last();
            let f_d = fwd.map(|m| m.start()).unwrap_or(usize::MAX);
            let b_d = bwd.map(|m| bseg.len() - m.end()).unwrap_or(usize::MAX);
            let pick = if f_d <= b_d {
                fwd.map(|m| &html[li..][m.start()..m.end()])
            } else {
                bwd.map(|m| &bseg[m.start()..m.end()])
            };
            if let Some(m) = pick {
                let cap = bp_attr("dateTime").captures(m).and_then(|c| c.get(1)).map(|g| g.as_str());
                last_reset_at = bp_iso(cap);
            }
        }
    }
    out["last_reset_at"] = last_reset_at.map(Value::String).unwrap_or(Value::Null);
    // 已排期重置卡：时刻/状态/链接都取卡片标记之后的第一个（防页面加第二个倒计时组件时
    // 全局首现被抢占）；状态为终态/负态时不算活信号——已执行由账本接住，取消/错过视同无信号
    if let Some(si) = html.find("product-tracking-scheduled-reset") {
        let after = &html[si..];
        let sf = bp_iso(bp_attr_get("targetIso", after));
        let pa = bp_iso(bp_attr_get("publishedAtIso", after));
        if sf.is_some() || pa.is_some() {
            let seg = &html[si..(si + 8000).min(html.len())];
            let status = bp_attr_get("data-signal-reset-status", seg);
            if !status.is_some_and(|s| BP_STATUS_BAD.is_match(s)) {
                let url = BP_X_URL
                    .find(seg)
                    .map(|m| format!("https://{}", m.as_str()));
                out["commitment"] = json!({
                    "scheduled_for": sf,
                    "posted_at": pa,
                    "url": url,
                    "text": "scheduled reset",
                });
            }
        }
    }
    // 至少要有一个可推导字段：标记还在但内容结构改版时，返回 null 走降级链，
    // 不把"什么都没解析到"伪装成"无重置预告"（也防止镜像写出空信封阻断 JSON 兜底）
    if out["last_reset_at"].is_null() && out.get("commitment").is_none() {
        return None;
    }
    Some(out)
}

// ── 副行文案（双语：产出 {zh, en}，widget 按界面语言挑选）──
// 星期/今天/明天一律在用户本地时区判定；目标时刻是 UTC 窗口起点。
fn sub_line_in(lang: &str, kind: &str, detail: &Map<String, Value>, now: i64, tz: Option<&str>) -> String {
    let zh = lang == "zh";
    let cn = |dow: u32| CN_DAY[dow as usize];
    let en = |dow: u32| EN_DAY[dow as usize];

    match kind {
        "happy" => {
            // 精确时间 → 倒计时（>48h 按天，1–48h 按小时，<1h 快到了）
            if let Some(t) = parse_ms(detail.get("scheduledISO")) {
                if t > now {
                    let h = (t - now) as f64 / HOUR as f64;
                    return if h < 1.0 {
                        if zh { "即将重置".into() } else { "reset imminent".into() }
                    } else if h < 48.0 {
                        let h = h.round() as i64;
                        if zh { format!("{h} 小时后重置") } else { format!("reset in ~{h}h") }
                    } else {
                        let d = (h / 24.0).round() as i64;
                        if zh { format!("{d} 天后重置") } else { format!("reset in {d} day{}", if d == 1 { "" } else { "s" }) }
                    };
                }
                // 过点未落地一律"即将"：迟到是常态，宽限期内不换个说法吓人
                return if zh { "即将重置".into() } else { "reset imminent".into() };
            }
            // 窗口制 → 窗口最晚边的本地星期（hedge 口径，学 codex-reset.com）
            if let Some(we) = parse_ms(detail.get("windowEnd")) {
                if we <= now {
                    return if zh { "即将重置".into() } else { "reset imminent".into() }; // 窗口已过未确认 → 迟到中
                }
                if let Some((_, dow)) = local_ymd_w(we, tz) {
                    return if zh { format!("最晚周{}重置", cn(dow)) } else { format!("reset by {}", en(dow)) };
                }
            }
            // 暗示/承诺：推文日子（PT 口径）+ UTC 窗口起点 → 本地星期/今天/明天
            if let Some(ts) = parse_ms(detail.get("targetStart")) {
                let te = parse_ms(detail.get("targetEnd"));
                if te.is_some_and(|e| ts <= now && now < e) {
                    return if zh { "即将重置".into() } else { "reset imminent".into() }; // 已进入预告窗口
                }
                if te.is_some_and(|e| e <= now) {
                    return if zh { "即将重置".into() } else { "reset imminent".into() }; // 窗口已过 → 迟到中
                }
                if let (Some((tgt, dow)), Some((cur, _)), Some((nxt, _))) =
                    (local_ymd_w(ts, tz), local_ymd_w(now, tz), local_ymd_w(now + DAY, tz))
                {
                    // 相对词按用户本地日期说：目标窗口落在本地今天/明天就直说，其余报星期
                    return if tgt == cur {
                        if zh { "预计今天重置".into() } else { "reset expected today".into() }
                    } else if tgt == nxt {
                        if zh { "预计明天重置".into() } else { "reset expected tomorrow".into() }
                    } else if zh {
                        format!("预计周{}重置", cn(dow))
                    } else {
                        format!("reset expected {}", en(dow))
                    };
                }
            }
            if detail.get("confirmed").is_some() {
                return if zh { "已预告重置".into() } else { "reset announced".into() };
            }
            if detail.get("teaseTier").is_some() {
                return if zh { "有重置暗示".into() } else { "reset hinted".into() };
            }
            // 近期重置过（无预告的 happy）：落地后按 reset_type 分开说——发卡 vs 用量直充；未知走通用
            if let Some(ds) = detail.get("daysSince").and_then(|d| d.as_f64()) {
                let days = ds.floor() as i64;
                let (just, ago): (&str, Box<dyn Fn(i64) -> String>) = match detail.get("resetType").and_then(|r| r.as_str()) {
                    Some("banked") => (
                        if zh { "刚发了重置卡" } else { "card just issued" },
                        Box::new(move |d| if zh { format!("上次发卡 {d} 天前") } else { format!("card issued {d}d ago") }),
                    ),
                    Some("regular") => (
                        if zh { "用量刚重置" } else { "usage just reset" },
                        Box::new(move |d| if zh { format!("用量 {d} 天前重置") } else { format!("usage reset {d}d ago") }),
                    ),
                    _ => (
                        if zh { "刚刚重置" } else { "just reset" },
                        Box::new(move |d| if zh { format!("上次重置 {d} 天前") } else { format!("last reset {d}d ago") }),
                    ),
                };
                return if ds < 1.0 { just.into() } else { ago(days) };
            }
            if zh { "已预告重置".into() } else { "reset announced".into() }
        }
        "unhappy" => if zh { "暂无重置预告".into() } else { "no reset news".into() }, // 不数天数：避免与个人订阅自动重置周期混淆
        _ => if zh { "数据不可用".into() } else { "data unavailable".into() },
    }
}

// s = {kind, detail}；返回 {"zh": ..., "en": ...}
pub fn sub_line(s: &Value, now: i64, tz: Option<&str>) -> Value {
    let kind = s.get("kind").and_then(|k| k.as_str()).unwrap_or("offline");
    let empty = Map::new();
    let detail = s.get("detail").and_then(|d| d.as_object()).unwrap_or(&empty);
    json!({
        "zh": sub_line_in("zh", kind, detail, now, tz),
        "en": sub_line_in("en", kind, detail, now, tz),
    })
}

// 守护进程展示决策：新数据 > 12h 内缓存 > OFFLINE。cache = { state, at(ms) } | null
pub const CACHE_MAX_AGE_MS: i64 = 12 * 3600_000;
const SIGNAL_FIELDS: &[&str] = &["scheduledISO", "windowEnd", "targetStart", "teaseTier", "confirmed"];

pub fn resolve_display(fresh: Option<&Value>, cache: Option<&Value>, now: i64, tz: Option<&str>) -> (Value, Value) {
    if let Some(f) = fresh {
        if f.get("kind").and_then(|k| k.as_str()) != Some("offline") {
            return (f.clone(), json!({"state": f, "at": now}));
        }
    }
    let ok = cache.is_some_and(|c| {
        let kind = c.get("state").and_then(|s| s.get("kind")).and_then(|k| k.as_str());
        let has_detail = c.get("state").and_then(|s| s.get("detail")).is_some_and(|d| d.is_object());
        let at = c.get("at").and_then(|a| a.as_i64());
        matches!(kind, Some("happy") | Some("unhappy"))
            && has_detail
            && at.is_some_and(|a| now - a < CACHE_MAX_AGE_MS && now - a > -HOUR)
    });
    if !ok {
        return (
            json!({"kind": "offline", "detail": {"sub": {"zh": "数据不可用", "en": "data unavailable"}}}),
            cache.cloned().unwrap_or(Value::Null),
        );
    }
    let c = cache.unwrap();
    let s = c.get("state").cloned().unwrap_or(Value::Null);
    let detail_v = s.get("detail").cloned().unwrap_or(json!({}));
    let detail = detail_v.as_object().cloned().unwrap_or_default();
    let last = parse_ms(detail_v.get("lastResetISO"));
    let has_signal = SIGNAL_FIELDS.iter().any(|f| detail_v.get(*f).is_some_and(|v| !v.is_null()));
    if !has_signal && last.is_none() {
        return (s, c.clone());
    }
    let mut d2 = detail;
    if let Some(l) = last {
        d2.insert("daysSince".into(), json!(((now - l) as f64 / DAY as f64).max(0.0)));
    }
    let kind = if has_signal {
        s.get("kind").and_then(|k| k.as_str()).unwrap_or("unhappy").to_string()
    } else {
        let ds = d2.get("daysSince").and_then(|d| d.as_f64());
        if ds.is_some_and(|d| d <= UNHAPPY_AFTER_DAYS) {
            "happy".to_string()
        } else {
            "unhappy".to_string()
        }
    };
    let interim = json!({"kind": kind, "detail": Value::Object(d2.clone())});
    d2.insert("sub".into(), sub_line(&interim, now, tz));
    (json!({"kind": kind, "detail": Value::Object(d2)}), c.clone())
}
