// 状态推导 + 文案 的规格测试（test/state.test.mjs 的 Rust 移植，逐条对齐）。
// now 固定为 2026-09-20T12:00:00Z（周日）。时区通过 sub_line 的 tz 参数显式传入，不依赖系统 TZ。
use is_tibo_happy::state::*;
use serde_json::{json, Value};

const H: i64 = 3600_000;
const D: i64 = 86400_000;

fn ms(s: &str) -> i64 {
    parse_iso_pub(s).unwrap()
}
fn iso(ms: i64) -> String {
    iso_pub(ms)
}
const NOW_MS: i64 = 1789905600000; // 2026-09-20T12:00:00Z (Sunday)

const SH: &str = "Asia/Shanghai";
const LA: &str = "America/Los_Angeles";
const UTC: &str = "UTC";
const NZ: &str = "Pacific/Auckland";

const LAST: &str = "2026-09-12T08:09:17.000Z"; // 8.3 天前
fn window() -> Value {
    json!({"start_hour": 23, "end_hour": 2, "label": "11 PM - 2 AM", "timezone": "UTC"})
}
fn tease(quote: Value, at: &str, expires: Value) -> Value {
    json!({"tier": "T1", "post": {"at": at, "quote": quote, "url": "https://x.com/x/status/1"}, "expires_at": expires})
}
fn tease_default(quote: Value) -> Value {
    tease(
        quote,
        "2026-09-19T16:48:38.000Z",
        json!("2026-09-21T16:48:38.000Z"),
    )
}

fn run(j: &Value, tz: &str, now: i64) -> Value {
    let s = derive(j, now);
    let sub = sub_line(&s, now, Some(tz));
    json!({"kind": s["kind"], "zh": sub["zh"], "en": sub["en"]})
}

// ───────────── A. 暗示级 + 星期几 → 按 UTC 惯用窗口起点换算到用户本地星期 ─────────────
#[test]
fn tease_tuesday_local_weekday_differs_by_timezone() {
    let j = json!({"last_reset_at": LAST, "tease_signal": tease_default(json!("OK fine. But it’s also still coming in Tuesday")), "time_window": window()});
    // 窗口起点 2026-09-22T23:00Z：上海=周三 07:00，洛杉矶=周二 16:00，UTC=周二 23:00，奥克兰=周三 11:00
    assert_eq!(
        run(&j, SH, NOW_MS),
        json!({"kind":"happy","zh":"预计周三重置","en":"reset expected Wednesday"})
    );
    assert_eq!(
        run(&j, LA, NOW_MS),
        json!({"kind":"happy","zh":"预计周二重置","en":"reset expected Tuesday"})
    );
    assert_eq!(
        run(&j, UTC, NOW_MS),
        json!({"kind":"happy","zh":"预计周二重置","en":"reset expected Tuesday"})
    );
    assert_eq!(
        run(&j, NZ, NOW_MS),
        json!({"kind":"happy","zh":"预计周三重置","en":"reset expected Wednesday"})
    );
}

#[test]
fn tease_tuesday_without_time_window_default_23utc() {
    let j =
        json!({"last_reset_at": LAST, "tease_signal": tease_default(json!("coming in Tuesday"))});
    assert_eq!(
        run(&j, SH, NOW_MS),
        json!({"kind":"happy","zh":"预计周三重置","en":"reset expected Wednesday"})
    );
}

// ───────────── B. today / tomorrow 相对词 ─────────────
#[test]
fn tease_today_posted_this_morning() {
    let j = json!({"last_reset_at": LAST, "tease_signal": tease(json!("reset coming today"), "2026-09-20T10:00:00Z", json!("2026-09-22T10:00:00Z")), "time_window": window()});
    assert_eq!(
        run(&j, UTC, NOW_MS),
        json!({"kind":"happy","zh":"预计今天重置","en":"reset expected today"})
    );
    assert_eq!(
        run(&j, SH, NOW_MS),
        json!({"kind":"happy","zh":"预计明天重置","en":"reset expected tomorrow"})
    );
}

#[test]
fn tease_tomorrow_post_date_plus_1() {
    let j = json!({"last_reset_at": LAST, "tease_signal": tease(json!("tomorrow for sure"), "2026-09-20T10:00:00Z", json!("2026-09-22T10:00:00Z")), "time_window": window()});
    assert_eq!(
        run(&j, UTC, NOW_MS),
        json!({"kind":"happy","zh":"预计明天重置","en":"reset expected tomorrow"})
    );
    assert_eq!(
        run(&j, SH, NOW_MS),
        json!({"kind":"happy","zh":"预计周二重置","en":"reset expected Tuesday"})
    );
}

// ───────────── B2. 相对词以 PT 日期为基准 ─────────────
#[test]
fn post_at_pt_monday_evening_tomorrow_is_pt_tuesday() {
    let now = ms("2026-09-22T01:00:00Z");
    let j = json!({"last_reset_at": LAST, "tease_signal": tease(json!("tomorrow!"), "2026-09-22T00:30:00Z", json!("2026-09-24T00:00:00Z")), "time_window": window()});
    assert_eq!(
        run(&j, LA, now),
        json!({"kind":"happy","zh":"预计明天重置","en":"reset expected tomorrow"})
    );
    assert_eq!(
        run(&j, UTC, now),
        json!({"kind":"happy","zh":"预计今天重置","en":"reset expected today"})
    );
}

#[test]
fn post_at_pt_tuesday_evening_tuesday_is_same_pt_tuesday() {
    let now = ms("2026-09-23T01:00:00Z");
    let j = json!({"last_reset_at": LAST, "tease_signal": tease(json!("coming in Tuesday"), "2026-09-23T00:30:00Z", json!("2026-09-25T00:00:00Z")), "time_window": window()});
    assert_eq!(
        run(&j, LA, now),
        json!({"kind":"happy","zh":"即将重置","en":"reset imminent"})
    );
    assert_eq!(
        run(&j, SH, now),
        json!({"kind":"happy","zh":"即将重置","en":"reset imminent"})
    );
}

#[test]
fn tease_post_beyond_window_plus_grace_is_stale() {
    let now = ms("2026-09-25T12:00:00Z");
    let j = json!({"last_reset_at": LAST, "tease_signal": tease(json!("coming in Tuesday"), "2026-09-19T16:48:38Z", Value::Null)});
    assert_eq!(run(&j, UTC, now)["kind"], "unhappy");
    let fresh = json!({"last_reset_at": LAST, "tease_signal": tease(json!("coming in Friday"), "2026-09-23T16:48:38Z", Value::Null)});
    assert_eq!(run(&fresh, UTC, now)["kind"], "happy");
}

#[test]
fn explicit_expires_at_beats_72h_default() {
    let now = ms("2026-09-23T12:00:00Z");
    let j = json!({"last_reset_at": LAST, "tease_signal": tease(json!("soon™"), "2026-09-19T16:48:38Z", json!("2026-09-24T00:00:00Z"))});
    assert_eq!(
        run(&j, UTC, now),
        json!({"kind":"happy","zh":"有重置暗示","en":"reset hinted"})
    );
}

#[test]
fn weekend_tonight_resolve_never_throw() {
    let j = |w: &str| json!({"last_reset_at": LAST, "tease_signal": tease(json!(w), "2026-09-20T10:00:00Z", json!("2026-09-27T00:00:00Z")), "time_window": window()});
    assert_eq!(
        run(&j("this weekend"), UTC, NOW_MS),
        json!({"kind":"happy","zh":"预计周六重置","en":"reset expected Saturday"})
    );
    assert_eq!(
        run(&j("tonight probably"), UTC, NOW_MS),
        json!({"kind":"happy","zh":"预计今天重置","en":"reset expected today"})
    );
}

#[test]
fn invalid_tz_degrades_to_hinted() {
    let j = json!({"last_reset_at": LAST, "tease_signal": tease_default(json!("coming in Tuesday")), "time_window": window()});
    assert_eq!(
        run(&j, "Mars/Olympus", NOW_MS),
        json!({"kind":"happy","zh":"有重置暗示","en":"reset hinted"})
    );
}

#[test]
fn detail_carries_no_volatile_timestamp() {
    let j = json!({"last_reset_at": LAST, "tease_signal": tease_default(json!("coming in Tuesday")), "time_window": window()});
    assert_eq!(derive(&j, NOW_MS), derive(&j, NOW_MS + 60_000));
}

// ───────────── C/D. 暗示过期 / 窗口已过但暗示仍在 ─────────────
#[test]
fn expired_tease_ignored_after_grace() {
    let now = ms("2026-09-24T15:00:00Z");
    let j = json!({"last_reset_at": LAST, "tease_signal": tease_default(json!("coming in Tuesday")), "time_window": window()});
    assert_eq!(
        run(&j, UTC, now),
        json!({"kind":"unhappy","zh":"暂无重置预告","en":"no reset news"})
    );
}

#[test]
fn tease_live_window_passed_imminent() {
    let now = ms("2026-09-23T12:00:00Z");
    let j = json!({"last_reset_at": LAST, "tease_signal": tease(json!("coming in Tuesday"), "2026-09-19T16:48:38Z", json!("2026-09-24T00:00:00Z")), "time_window": window()});
    assert_eq!(
        run(&j, UTC, now),
        json!({"kind":"happy","zh":"即将重置","en":"reset imminent"})
    );
}

#[test]
fn tease_unparseable_quote_hinted() {
    let j = json!({"last_reset_at": LAST, "tease_signal": tease_default(json!("soon™"))});
    assert_eq!(
        run(&j, UTC, NOW_MS),
        json!({"kind":"happy","zh":"有重置暗示","en":"reset hinted"})
    );
}

#[test]
fn expires_at_past_dies_only_without_other_reasons() {
    let live = json!({"last_reset_at": LAST, "tease_signal": tease(json!("coming in Tuesday"), "2026-09-19T16:48:38Z", json!(iso(NOW_MS)))});
    assert_eq!(run(&live, UTC, NOW_MS)["kind"], "happy");
    let dead = json!({"last_reset_at": LAST, "tease_signal": tease(json!("soon™"), "2026-09-15T16:48:38Z", json!(iso(NOW_MS - H)))});
    assert_eq!(run(&dead, UTC, NOW_MS)["kind"], "unhappy");
}

// ───────────── I. 弱信号兜底 latest_hint ─────────────
#[test]
fn dropped_tease_signal_but_latest_hint_fresh() {
    let j = json!({
        "last_reset_at": LAST,
        "latest_hint": {"id": "x", "at": "2026-09-19T16:48:38.000Z", "url": "https://x.com/x/1", "quote": "OK fine. But it’s also still coming in Tuesday", "day": null},
        "time_window": window(),
    });
    let now = ms("2026-09-22T02:00:00Z");
    assert_eq!(
        run(&j, UTC, now),
        json!({"kind":"happy","zh":"预计今天重置","en":"reset expected today"})
    );
    assert_eq!(
        run(&j, SH, now),
        json!({"kind":"happy","zh":"预计明天重置","en":"reset expected tomorrow"})
    );
}

#[test]
fn hint_window_passed_within_grace_imminent() {
    let j = json!({"last_reset_at": LAST, "latest_hint": {"at": "2026-09-19T16:48:38.000Z", "quote": "coming in Tuesday"}, "time_window": window()});
    let now = ms("2026-09-23T12:00:00Z");
    assert_eq!(
        run(&j, UTC, now),
        json!({"kind":"happy","zh":"即将重置","en":"reset imminent"})
    );
}

#[test]
fn hint_fully_stale_unhappy() {
    let j = json!({"last_reset_at": LAST, "latest_hint": {"at": "2026-09-19T16:48:38.000Z", "quote": "coming in Tuesday"}, "time_window": window()});
    let now = ms("2026-09-24T15:00:00Z");
    assert_eq!(
        run(&j, UTC, now),
        json!({"kind":"unhappy","zh":"暂无重置预告","en":"no reset news"})
    );
}

#[test]
fn hint_unparseable_quote_not_signal() {
    let j = json!({"last_reset_at": LAST, "latest_hint": {"at": "2026-09-19T16:48:38.000Z", "quote": "hmm interesting"}});
    assert_eq!(run(&j, UTC, NOW_MS)["kind"], "unhappy");
}

#[test]
fn hint_older_than_7d_ignored() {
    let j = json!({"last_reset_at": LAST, "latest_hint": {"at": "2026-09-10T16:48:38.000Z", "quote": "coming in Tuesday"}});
    assert_eq!(run(&j, UTC, NOW_MS)["kind"], "unhappy");
}

#[test]
fn reset_landed_after_tease_fulfilled() {
    let j = json!({
        "last_reset_at": "2026-09-22T23:30:00.000Z",
        "tease_signal": tease(json!("coming in Tuesday"), "2026-09-19T16:48:38Z", json!("2026-09-25T00:00:00Z")),
        "time_window": window(),
    });
    let now = ms("2026-09-23T01:00:00Z");
    assert_eq!(
        run(&j, UTC, now),
        json!({"kind":"happy","zh":"刚刚重置","en":"just reset"})
    );
}

#[test]
fn tease_posted_after_last_reset_live() {
    let j = json!({"last_reset_at": "2026-09-18T08:00:00Z", "tease_signal": tease_default(json!("coming in Tuesday")), "time_window": window()});
    assert_eq!(
        run(&j, LA, NOW_MS),
        json!({"kind":"happy","zh":"预计周二重置","en":"reset expected Tuesday"})
    );
}

#[test]
fn resets_scheduled_for_overdue_within_grace_imminent() {
    let j = json!({"scheduled": {"scheduled_for": iso(NOW_MS - 8 * H), "display_text": "x", "announced_at": iso(NOW_MS - 2 * D)}, "events": [{"announced_at": LAST}]});
    assert_eq!(
        run(&j, UTC, NOW_MS),
        json!({"kind":"happy","zh":"即将重置","en":"reset imminent"})
    );
    let done = json!({"scheduled": {"scheduled_for": "2026-09-19T23:00:00Z", "display_text": "x", "announced_at": "2026-09-18T00:00:00Z"}, "events": [{"announced_at": "2026-09-20T01:00:00Z"}]});
    assert_eq!(
        run(&done, UTC, NOW_MS),
        json!({"kind":"happy","zh":"刚刚重置","en":"just reset"})
    );
}

// ───────────── E. 明确承诺 → 倒计时 ─────────────
#[test]
fn commitment_countdown_granularity() {
    let c =
        |t: i64| json!({"last_reset_at": LAST, "commitment": {"scheduled_for": iso(NOW_MS + t)}});
    assert_eq!(
        run(&c((6.2 * D as f64) as i64), UTC, NOW_MS),
        json!({"kind":"happy","zh":"6 天后重置","en":"reset in 6 days"})
    );
    assert_eq!(
        run(&c((1.4 * D as f64) as i64), UTC, NOW_MS),
        json!({"kind":"happy","zh":"34 小时后重置","en":"reset in ~34h"})
    );
    assert_eq!(
        run(&c(3 * H), UTC, NOW_MS),
        json!({"kind":"happy","zh":"3 小时后重置","en":"reset in ~3h"})
    );
    assert_eq!(
        run(&c(30 * 60_000), UTC, NOW_MS),
        json!({"kind":"happy","zh":"即将重置","en":"reset imminent"})
    );
    assert_eq!(
        run(&c(D), UTC, NOW_MS),
        json!({"kind":"happy","zh":"24 小时后重置","en":"reset in ~24h"})
    );
    assert_eq!(
        run(&c(2 * D), UTC, NOW_MS),
        json!({"kind":"happy","zh":"2 天后重置","en":"reset in 2 days"})
    );
    assert_eq!(
        run(&c(D + D / 2), UTC, NOW_MS),
        json!({"kind":"happy","zh":"36 小时后重置","en":"reset in ~36h"})
    );
}

#[test]
fn commitment_just_passed_imminent_long_passed_still_imminent() {
    let c =
        |t: i64| json!({"last_reset_at": LAST, "commitment": {"scheduled_for": iso(NOW_MS + t)}});
    assert_eq!(
        run(&c(-60_000), UTC, NOW_MS),
        json!({"kind":"happy","zh":"即将重置","en":"reset imminent"})
    );
    assert_eq!(
        run(&c(-7 * H), UTC, NOW_MS),
        json!({"kind":"happy","zh":"即将重置","en":"reset imminent"})
    );
}

#[test]
fn commitment_overdue_beyond_grace_stale() {
    let j = json!({"last_reset_at": LAST, "commitment": {"scheduled_for": iso(NOW_MS - 2 * D)}});
    assert_eq!(
        run(&j, UTC, NOW_MS),
        json!({"kind":"unhappy","zh":"暂无重置预告","en":"no reset news"})
    );
}

#[test]
fn commitment_beats_tease() {
    let j = json!({"last_reset_at": LAST, "commitment": {"scheduled_for": iso(NOW_MS + 5 * D)}, "tease_signal": tease_default(json!("coming in Tuesday"))});
    assert_eq!(
        run(&j, UTC, NOW_MS),
        json!({"kind":"happy","zh":"5 天后重置","en":"reset in 5 days"})
    );
}

// ───────────── E2. 承诺级信号的锚定规则 ─────────────
#[test]
fn commitment_without_anchor_not_signal() {
    let j = json!({"last_reset_at": LAST, "commitment": {"text": "reset coming eventually"}});
    assert_eq!(run(&j, UTC, NOW_MS)["kind"], "unhappy");
    assert_eq!(run(&j, UTC, NOW_MS + 90 * D)["kind"], "unhappy");
}

#[test]
fn official_signal_no_post_time_no_reanchor() {
    let j = json!({"last_reset_at": LAST, "official_signal": {"text": "by Tuesday"}});
    assert_eq!(run(&j, UTC, NOW_MS)["kind"], "unhappy");
    assert_eq!(run(&j, UTC, NOW_MS + 90 * D)["kind"], "unhappy");
}

#[test]
fn official_signal_with_posted_at_anchors() {
    let j = json!({"last_reset_at": LAST, "official_signal": {"text": "by Tuesday", "posted_at": "2026-09-19T16:48:38.000Z"}, "time_window": window()});
    assert_eq!(
        run(&j, LA, NOW_MS),
        json!({"kind":"happy","zh":"预计周二重置","en":"reset expected Tuesday"})
    );
    let old = json!({"last_reset_at": LAST, "official_signal": {"text": "by Tuesday", "posted_at": "2026-09-10T16:48:38.000Z"}});
    assert_eq!(run(&old, UTC, NOW_MS)["kind"], "unhappy");
}

#[test]
fn non_string_quote_display_text_never_throws() {
    let j = json!({"last_reset_at": LAST, "tease_signal": tease_default(json!(42))});
    assert_eq!(
        run(&j, UTC, NOW_MS),
        json!({"kind":"happy","zh":"有重置暗示","en":"reset hinted"})
    );
    let hint = json!({"last_reset_at": LAST, "latest_hint": {"at": "2026-09-19T16:48:38.000Z", "quote": {"text": "x"}}});
    assert_eq!(run(&hint, UTC, NOW_MS)["kind"], "unhappy");
    let r = json!({"scheduled": {"display_text": {"text": "soon"}, "announced_at": "2026-09-19T16:48:38.000Z"}, "events": [{"announced_at": LAST}]});
    assert_eq!(run(&r, UTC, NOW_MS)["kind"], "unhappy");
}

// ───────────── F. 窗口制 ─────────────
#[test]
fn teased_window_end_by_local_weekday() {
    let j = json!({"last_reset_at": LAST, "official_signal": {"text": "by Tuesday"}, "teased_window": {"start": "2026-09-22T23:00:00Z", "end": "2026-09-23T02:00:00Z"}});
    assert_eq!(
        run(&j, LA, NOW_MS),
        json!({"kind":"happy","zh":"最晚周二重置","en":"reset by Tuesday"})
    );
    assert_eq!(
        run(&j, SH, NOW_MS),
        json!({"kind":"happy","zh":"最晚周三重置","en":"reset by Wednesday"})
    );
}

#[test]
fn teased_window_alone_counts_overdue_imminent() {
    let j = json!({"last_reset_at": LAST, "teased_window": {"start": "2026-09-22T23:00:00Z", "end": "2026-09-23T02:00:00Z"}});
    assert_eq!(
        run(&j, LA, NOW_MS),
        json!({"kind":"happy","zh":"最晚周二重置","en":"reset by Tuesday"})
    );
    let late = ms("2026-09-24T12:00:00Z");
    assert_eq!(
        run(&j, LA, late),
        json!({"kind":"happy","zh":"即将重置","en":"reset imminent"})
    );
    let dead = ms("2026-09-25T12:00:00Z");
    assert_eq!(run(&j, LA, dead)["kind"], "unhappy");
}

#[test]
fn teased_window_malformed_dropped_anchorless_not_signal() {
    let j = json!({"last_reset_at": LAST, "official_signal": {"text": "by Tuesday"}, "teased_window": {"start": "2026-09-23T02:00:00Z", "end": "2026-09-22T23:00:00Z"}});
    assert_eq!(
        run(&j, UTC, NOW_MS),
        json!({"kind":"unhappy","zh":"暂无重置预告","en":"no reset news"})
    );
}

// ───────────── G. 账本 ─────────────
#[test]
fn ledger_recent_happy_over_3d_unhappy() {
    let l = |d: f64| json!({"last_reset_at": iso(NOW_MS - (d * D as f64) as i64)});
    assert_eq!(
        run(&l(0.5), UTC, NOW_MS),
        json!({"kind":"happy","zh":"刚刚重置","en":"just reset"})
    );
    assert_eq!(
        run(&l(2.1), UTC, NOW_MS),
        json!({"kind":"happy","zh":"上次重置 2 天前","en":"last reset 2d ago"})
    );
    assert_eq!(
        run(&l(3.0), UTC, NOW_MS),
        json!({"kind":"happy","zh":"上次重置 3 天前","en":"last reset 3d ago"})
    );
    assert_eq!(
        run(&l(3.01), UTC, NOW_MS),
        json!({"kind":"unhappy","zh":"暂无重置预告","en":"no reset news"})
    );
    assert_eq!(
        run(&l(30.0), UTC, NOW_MS),
        json!({"kind":"unhappy","zh":"暂无重置预告","en":"no reset news"})
    );
}

#[test]
fn ledger_future_last_reset_just_reset() {
    assert_eq!(
        run(&json!({"last_reset_at": iso(NOW_MS + D)}), UTC, NOW_MS),
        json!({"kind":"happy","zh":"刚刚重置","en":"just reset"})
    );
}

// ───────────── H. 垃圾输入 ─────────────
#[test]
fn garbage_upstream_never_throws() {
    assert_eq!(derive(&Value::Null, NOW_MS)["kind"], "offline");
    assert_eq!(
        derive(&json!("<html>cloudflare</html>"), NOW_MS)["kind"],
        "offline"
    );
    assert_eq!(derive(&json!([]), NOW_MS)["kind"], "offline");
    assert_eq!(derive(&json!(42), NOW_MS)["kind"], "offline");
    assert_eq!(
        run(&json!({}), UTC, NOW_MS),
        json!({"kind":"unhappy","zh":"暂无重置预告","en":"no reset news"})
    );
    assert_eq!(
        run(&json!({"last_reset_at": "garbage"}), UTC, NOW_MS),
        json!({"kind":"unhappy","zh":"暂无重置预告","en":"no reset news"})
    );
    assert_eq!(
        run(&json!({"age_days": "8"}), UTC, NOW_MS)["kind"],
        "unhappy"
    );
    assert_eq!(
        run(
            &json!({"last_reset_at": LAST, "tease_signal": "not-an-object"}),
            UTC,
            NOW_MS
        )["kind"],
        "unhappy"
    );
    assert_eq!(
        run(
            &json!({"last_reset_at": LAST, "tease_signal": {"post": null}}),
            UTC,
            NOW_MS
        )["kind"],
        "unhappy"
    );
    assert_eq!(
        run(
            &json!({"last_reset_at": LAST, "commitment": "yes"}),
            UTC,
            NOW_MS
        )["kind"],
        "unhappy"
    );
    assert_eq!(
        run(
            &json!({"last_reset_at": LAST, "time_window": {"start_hour": "x"}, "tease_signal": tease_default(json!("coming in Tuesday"))}),
            SH,
            NOW_MS
        )["zh"],
        "预计周三重置"
    );
    assert_eq!(
        sub_line(&json!({"kind": "offline", "detail": {}}), NOW_MS, None)["zh"],
        "数据不可用"
    );
}

// ───────────── K/L. 备用源 resets 形状 ─────────────
#[test]
fn resets_scheduled_tease_localized_weekday() {
    let j = json!({
        "scheduled": {"display_text": "@udi OK fine. But it’s also still coming in Tuesday", "announced_at": "2026-09-19T16:48:38.000Z", "scheduled_for": null, "reset_type": "banked"},
        "events": [{"announced_at": LAST, "reset_type": "regular"}],
    });
    assert_eq!(
        run(&j, LA, NOW_MS),
        json!({"kind":"happy","zh":"预计周二重置","en":"reset expected Tuesday"})
    );
    assert_eq!(
        run(&j, SH, NOW_MS),
        json!({"kind":"happy","zh":"预计周三重置","en":"reset expected Wednesday"})
    );
}

#[test]
fn resets_scheduled_for_exact_countdown() {
    let j = json!({"scheduled": {"scheduled_for": iso(NOW_MS + 4 * D), "display_text": "x"}, "events": []});
    assert_eq!(
        run(&j, UTC, NOW_MS),
        json!({"kind":"happy","zh":"4 天后重置","en":"reset in 4 days"})
    );
}

#[test]
fn resets_bare_string_scheduled() {
    assert_eq!(
        run(
            &json!({"scheduled": "2026-09-25T23:00:00.000Z", "events": [{"announced_at": LAST}]}),
            UTC,
            NOW_MS
        ),
        json!({"kind":"happy","zh":"5 天后重置","en":"reset in 5 days"})
    );
    assert_eq!(
        run(
            &json!({"scheduled": "coming Tuesday", "events": [{"announced_at": LAST}]}),
            UTC,
            NOW_MS
        )["kind"],
        "unhappy"
    );
    assert_eq!(
        run(
            &json!({"scheduled": "2026-09-15T23:00:00.000Z", "events": [{"announced_at": LAST}]}),
            UTC,
            NOW_MS
        )["kind"],
        "unhappy"
    );
}

#[test]
fn resets_stale_scheduled_ignored() {
    let now = ms("2026-09-25T12:00:00Z");
    let j = json!({"scheduled": {"display_text": "coming in Tuesday", "announced_at": "2026-09-19T16:48:38.000Z", "scheduled_for": null}, "events": [{"announced_at": LAST}]});
    assert_eq!(
        run(&j, UTC, now),
        json!({"kind":"unhappy","zh":"暂无重置预告","en":"no reset news"})
    );
}

#[test]
fn resets_ledger_rules_match_forecast() {
    assert_eq!(
        run(
            &json!({"events": [{"announced_at": iso(NOW_MS - D)}]}),
            UTC,
            NOW_MS
        ),
        json!({"kind":"happy","zh":"上次重置 1 天前","en":"last reset 1d ago"})
    );
    // 落地后按 reset_type 分开描述：发卡型 vs 用量直充型；未知类型走通用文案
    assert_eq!(
        run(
            &json!({"events": [{"announced_at": iso(NOW_MS - D / 2), "reset_type": "banked"}]}),
            UTC,
            NOW_MS
        ),
        json!({"kind":"happy","zh":"刚发了重置卡","en":"card just issued"})
    );
    assert_eq!(
        run(
            &json!({"events": [{"announced_at": iso(NOW_MS - 2 * D), "reset_type": "banked"}]}),
            UTC,
            NOW_MS
        ),
        json!({"kind":"happy","zh":"上次发卡 2 天前","en":"card issued 2d ago"})
    );
    assert_eq!(
        run(
            &json!({"events": [{"announced_at": iso(NOW_MS - D / 2), "reset_type": "regular"}]}),
            UTC,
            NOW_MS
        ),
        json!({"kind":"happy","zh":"用量刚重置","en":"usage just reset"})
    );
    assert_eq!(
        run(
            &json!({"events": [{"announced_at": iso(NOW_MS - D), "reset_type": "regular"}]}),
            UTC,
            NOW_MS
        ),
        json!({"kind":"happy","zh":"用量 1 天前重置","en":"usage reset 1d ago"})
    );
    assert_eq!(
        run(
            &json!({"events": [{"announced_at": iso(NOW_MS - D), "reset_type": "weird"}]}),
            UTC,
            NOW_MS
        ),
        json!({"kind":"happy","zh":"上次重置 1 天前","en":"last reset 1d ago"})
    );
    // 取最近一次事件的类型；更老的事件类型不冒名顶替
    assert_eq!(
        run(
            &json!({"events": [{"announced_at": iso(NOW_MS - 3 * D), "reset_type": "banked"}, {"announced_at": iso(NOW_MS - D / 2), "reset_type": "regular"}]}),
            UTC,
            NOW_MS
        ),
        json!({"kind":"happy","zh":"用量刚重置","en":"usage just reset"})
    );
    assert_eq!(
        run(&json!({"events": [{"announced_at": LAST}]}), UTC, NOW_MS),
        json!({"kind":"unhappy","zh":"暂无重置预告","en":"no reset news"})
    );
    assert_eq!(
        run(&json!({"events": []}), UTC, NOW_MS),
        json!({"kind":"unhappy","zh":"暂无重置预告","en":"no reset news"})
    );
    assert_eq!(
        run(&json!({"events": [{"announced_at": "bad"}]}), UTC, NOW_MS),
        json!({"kind":"unhappy","zh":"暂无重置预告","en":"no reset news"})
    );
}

#[test]
fn derive_dispatches_on_shape() {
    assert_eq!(
        derive_forecast(&json!({"last_reset_at": LAST}), NOW_MS)["kind"],
        "unhappy"
    );
    assert_eq!(
        derive_state(&json!({"events": [{"announced_at": LAST}]}), NOW_MS)["kind"],
        "unhappy"
    );
    assert_eq!(derive(&json!({"events": []}), NOW_MS)["kind"], "unhappy");
    assert_eq!(
        derive(&json!({"last_reset_at": LAST}), NOW_MS)["kind"],
        "unhappy"
    );
}

// ───────────── subLine 兜底 ─────────────
#[test]
fn sub_line_never_throws_on_partial_detail() {
    for kind in ["happy", "unhappy", "offline"] {
        let o = sub_line(&json!({"kind": kind, "detail": {}}), NOW_MS, None);
        assert!(o["zh"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(o["en"].as_str().is_some_and(|s| !s.is_empty()));
    }
}

// ───────────── resolveDisplay ─────────────
#[test]
fn resolve_fresh_wins_becomes_cache() {
    let fresh = json!({"kind": "happy", "detail": {"sub": {"zh": "a", "en": "a"}}});
    let cache = json!({"state": {"kind": "unhappy", "detail": {}}, "at": NOW_MS - H});
    let (s, c) = resolve_display(Some(&fresh), Some(&cache), NOW_MS, None);
    assert_eq!(s, fresh);
    assert_eq!(c, json!({"state": fresh, "at": NOW_MS}));
}

#[test]
fn resolve_fetch_failed_fresh_cache_kept() {
    let cached = json!({"kind": "happy", "detail": {"sub": {"zh": "预计周三重置", "en": "x"}}});
    let cache = json!({"state": cached, "at": NOW_MS - (11.9 * H as f64) as i64});
    let (s, c) = resolve_display(None, Some(&cache), NOW_MS, None);
    assert_eq!(s, cached);
    assert_eq!(c, cache); // cache 不被刷新
}

#[test]
fn resolve_stale_or_absent_cache_offline() {
    let cached = json!({"kind": "happy", "detail": {}});
    assert_eq!(
        resolve_display(
            None,
            Some(&json!({"state": cached, "at": NOW_MS - 12 * H})),
            NOW_MS,
            None
        )
        .0["kind"],
        "offline"
    );
    assert_eq!(
        resolve_display(None, None, NOW_MS, None).0["kind"],
        "offline"
    );
    assert_eq!(
        resolve_display(
            None,
            Some(&json!({"state": cached, "at": "garbage"})),
            NOW_MS,
            None
        )
        .0["kind"],
        "offline"
    );
    assert_eq!(
        resolve_display(
            None,
            Some(&json!({"state": "nope", "at": NOW_MS})),
            NOW_MS,
            None
        )
        .0["kind"],
        "offline"
    );
    let (o, _) = resolve_display(None, None, NOW_MS, None);
    assert_eq!(
        o["detail"]["sub"],
        json!({"zh": "数据不可用", "en": "data unavailable"})
    );
}

#[test]
fn resolve_cached_offline_never_kept() {
    let cache = json!({"state": {"kind": "offline", "detail": {}}, "at": NOW_MS});
    assert_eq!(
        resolve_display(None, Some(&cache), NOW_MS, None).0["kind"],
        "offline"
    );
}

#[test]
fn resolve_bad_at_rejected() {
    let good = json!({"kind": "happy", "detail": {"sub": {"zh": "x", "en": "x"}}});
    assert_eq!(
        resolve_display(
            None,
            Some(&json!({"state": good, "at": (NOW_MS - H).to_string()})),
            NOW_MS,
            None
        )
        .0["kind"],
        "offline"
    );
    assert_eq!(
        resolve_display(
            None,
            Some(&json!({"state": good, "at": f64::NAN})),
            NOW_MS,
            None
        )
        .0["kind"],
        "offline"
    );
    assert_eq!(
        resolve_display(None, Some(&json!({"state": good})), NOW_MS, None).0["kind"],
        "offline"
    );
    assert_eq!(
        resolve_display(
            None,
            Some(&json!({"state": good, "at": [1, 2]})),
            NOW_MS,
            None
        )
        .0["kind"],
        "offline"
    );
}

#[test]
fn resolve_future_at_no_immortality() {
    let good = json!({"kind": "happy", "detail": {"sub": {"zh": "x", "en": "x"}}});
    assert_eq!(
        resolve_display(
            None,
            Some(&json!({"state": good, "at": NOW_MS + 30 * 60_000})),
            NOW_MS,
            None
        )
        .0["kind"],
        "happy"
    );
    assert_eq!(
        resolve_display(
            None,
            Some(&json!({"state": good, "at": NOW_MS + 72 * H})),
            NOW_MS,
            None
        )
        .0["kind"],
        "offline"
    );
}

#[test]
fn resolve_old_kind_waiting_not_kept() {
    let waiting = json!({"state": {"kind": "waiting", "detail": {"sub": {"zh": "x", "en": "x"}}}, "at": NOW_MS - H});
    let nodetail = json!({"state": {"kind": "happy"}, "at": NOW_MS - H});
    assert_eq!(
        resolve_display(None, Some(&waiting), NOW_MS, None).0["kind"],
        "offline"
    );
    assert_eq!(
        resolve_display(None, Some(&nodetail), NOW_MS, None).0["kind"],
        "offline"
    );
}

#[test]
fn resolve_offline_fresh_does_not_overwrite() {
    let good = json!({"kind": "happy", "detail": {"sub": {"zh": "x", "en": "x"}}});
    let cache = json!({"state": good, "at": NOW_MS - H});
    let (s, c) = resolve_display(
        Some(&json!({"kind": "offline", "detail": {}})),
        Some(&cache),
        NOW_MS,
        None,
    );
    assert_eq!(s["kind"], "happy");
    assert_eq!(c["state"], good);
}

#[test]
fn resolve_cached_state_reworded_for_now() {
    // 缓存写于周一 20:00 UTC（窗口前）说"预计今天"；周二 06:00 回放（10h，有效）→ 窗口已过 → 即将重置
    let written = ms("2026-09-21T20:00:00Z");
    let mut s = derive(
        &json!({"last_reset_at": LAST, "tease_signal": tease(json!("coming today"), "2026-09-21T10:00:00Z", json!("2026-09-23T00:00:00Z")), "time_window": window()}),
        written,
    );
    s["detail"]["sub"] = sub_line(&s, written, Some(UTC));
    assert_eq!(s["detail"]["sub"]["zh"], "预计今天重置");
    let later = ms("2026-09-22T06:00:00Z");
    let cache = json!({"state": s, "at": written});
    let (r, _) = resolve_display(None, Some(&cache), later, Some(UTC));
    assert_eq!(r["kind"], "happy");
    assert_eq!(r["detail"]["sub"]["zh"], "即将重置");
    // 账龄型也要随时间走：缓存时 2.5 天，10 小时后仍 2 天
    let mut l = derive(
        &json!({"last_reset_at": iso(NOW_MS - (2.5 * D as f64) as i64)}),
        NOW_MS,
    );
    l["detail"]["sub"] = sub_line(&l, NOW_MS, None);
    let cache = json!({"state": l, "at": NOW_MS});
    assert_eq!(
        resolve_display(None, Some(&cache), NOW_MS + 10 * H, None).0["detail"]["sub"]["zh"],
        "上次重置 2 天前"
    );
}

// ───────────── M. 第一信源 betteropc.com ─────────────
struct Bp {
    pid: &'static str,
    today: &'static str,
    last: &'static str,
    card: bool,
    status: &'static str,
    sf: &'static str,
    pa: &'static str,
}
impl Default for Bp {
    fn default() -> Self {
        Bp {
            pid: "codex",
            today: "none",
            last: "2026-09-12T08:09:17.000Z",
            card: true,
            status: "scheduled",
            sf: "2026-09-22T07:00:00.000Z",
            pa: "2026-09-19T16:48:38.000Z",
        }
    }
}
fn bp(o: Bp) -> String {
    format!(
        "<!doctype html><html><head><title>Codex 重置信号监控</title></head><body>
<div class=\"product-tracking-card\" data-product-id=\"{}\"><div class=\"product-tracking-reset-overview-row\"><span data-reset-today-status=\"{}\" data-reset-today-date=\"2026-09-22\" data-product-updated-at=\"2026-09-22T02:08:25.309Z\">今日无重置</span>
<time class=\"product-tracking-last-confirmed-reset\" dateTime=\"{}\"></time></div>
{}
<script>self.__next_f.push([1,\"{{\\\"targetIso\\\":\\\"{}\\\",\\\"publishedAtIso\\\":\\\"{}\\\"}}\"])</script>
</body></html>",
        o.pid,
        o.today,
        o.last,
        if o.card {
            format!("<li class=\"product-tracking-scheduled-reset\"><span data-signal-reset-status=\"{}\">已排期</span><a href=\"https://x.com/thsottiaux/status/2101352781219258527\"></a></li>", o.status)
        } else {
            String::new()
        },
        o.sf,
        o.pa,
    )
}
fn run_bp(o: Bp, tz: &str, now: i64) -> Value {
    let s = derive(&parse_betteropc(&bp(o)).unwrap_or(Value::Null), now);
    let sub = sub_line(&s, now, Some(tz));
    json!({"kind": s["kind"], "zh": sub["zh"], "en": sub["en"]})
}

#[test]
fn betteropc_parses_dom_and_rsc() {
    let p = parse_betteropc(&bp(Bp::default())).unwrap();
    assert_eq!(p["source"], "betteropc");
    assert_eq!(p["last_reset_at"], "2026-09-12T08:09:17.000Z");
    assert_eq!(p["updated_at"], "2026-09-22T02:08:25.309Z");
    assert_eq!(p["today"], json!({"status": "none", "date": "2026-09-22"}));
    assert_eq!(p["commitment"]["scheduled_for"], "2026-09-22T07:00:00.000Z");
    assert_eq!(p["commitment"]["posted_at"], "2026-09-19T16:48:38.000Z");
    assert_eq!(
        p["commitment"]["url"],
        "https://x.com/thsottiaux/status/2101352781219258527"
    );
}

#[test]
fn betteropc_0700z_is_today_for_mainland_china() {
    let now = ms("2026-09-22T04:00:00Z");
    assert_eq!(
        run_bp(Bp::default(), SH, now),
        json!({"kind":"happy","zh":"3 小时后重置","en":"reset in ~3h"})
    );
    assert_eq!(
        run_bp(Bp::default(), SH, ms("2026-09-22T08:00:00Z")),
        json!({"kind":"happy","zh":"即将重置","en":"reset imminent"})
    );
    assert_eq!(
        run_bp(Bp::default(), SH, ms("2026-09-22T20:00:00Z")),
        json!({"kind":"happy","zh":"即将重置","en":"reset imminent"})
    );
}

#[test]
fn betteropc_scheduled_for_stale_falls_to_ledger() {
    let now = ms("2026-09-24T12:00:00Z");
    assert_eq!(
        run_bp(Bp::default(), UTC, now),
        json!({"kind":"unhappy","zh":"暂无重置预告","en":"no reset news"})
    );
}

#[test]
fn betteropc_reset_landed_fulfilled() {
    let now = ms("2026-09-22T08:00:00Z");
    let s = derive(
        &parse_betteropc(&bp(Bp {
            last: "2026-09-22T07:05:00.000Z",
            ..Default::default()
        }))
        .unwrap(),
        now,
    );
    assert_eq!(s["kind"], "happy");
    assert_eq!(
        sub_line(&s, now, Some(SH)),
        json!({"zh": "刚刚重置", "en": "just reset"})
    );
}

#[test]
fn betteropc_terminal_status_not_signal() {
    for status in [
        "executed",
        "cancelled",
        "missed",
        "done",
        "已取消",
        "已执行",
        "已重置",
    ] {
        let now = ms("2026-09-22T04:00:00Z");
        assert_eq!(
            run_bp(
                Bp {
                    status,
                    ..Default::default()
                },
                UTC,
                now
            )["kind"],
            "unhappy",
            "status={status}"
        );
    }
}

#[test]
fn betteropc_no_card_ledger_only() {
    let now = ms("2026-09-22T04:00:00Z");
    assert_eq!(
        run_bp(
            Bp {
                card: false,
                ..Default::default()
            },
            UTC,
            now
        )["kind"],
        "unhappy"
    );
    assert_eq!(
        run_bp(
            Bp {
                card: false,
                last: "2026-09-22T02:00:00.000Z",
                ..Default::default()
            },
            UTC,
            now
        ),
        json!({"kind":"happy","zh":"刚刚重置","en":"just reset"})
    );
    assert_eq!(
        run_bp(
            Bp {
                sf: "not-a-date",
                ..Default::default()
            },
            UTC,
            now
        ),
        json!({"kind":"happy","zh":"已预告重置","en":"reset announced"})
    );
    assert_eq!(
        run_bp(
            Bp {
                sf: "not-a-date",
                pa: "",
                ..Default::default()
            },
            UTC,
            now
        )["kind"],
        "unhappy"
    );
    assert_eq!(
        run_bp(
            Bp {
                pa: "",
                ..Default::default()
            },
            UTC,
            now
        )["kind"],
        "happy"
    );
}

#[test]
fn betteropc_garbage_null_never_throws() {
    assert!(parse_betteropc("<html>just a moment</html>").is_none());
    assert!(parse_betteropc("").is_none());
    assert!(parse_betteropc("{}").is_none());
    assert!(parse_betteropc(&bp(Bp {
        pid: "claude-code",
        ..Default::default()
    }))
    .is_none());
    // 正文提到 Codex 也不顶用：守卫按身份不认字样
    let tampered = bp(Bp::default()).replacen("data-product-id", "data-x-product-id", 1)
        + "<p>Codex 与 Claude Code 对比</p>";
    assert!(parse_betteropc(&tampered).is_none());
    let shell = "<div class=\"product-tracking-x\" data-product-id=\"codex\" data-reset-today-status=\"none\"></div>";
    assert!(parse_betteropc(shell).is_none());
    let bad = parse_betteropc(&bp(Bp {
        pa: "",
        last: "garbage",
        ..Default::default()
    }))
    .unwrap();
    assert!(bad["commitment"]["posted_at"].is_null());
    assert!(bad["last_reset_at"].is_null());
}

#[test]
fn betteropc_props_order_drift_no_neighbour_steal() {
    let reordered = "<div class=\"product-tracking-card\" data-product-id=\"codex\">\
        \"dateTime\":\"2026-09-12T08:09:17.000Z\",\"className\":\"product-tracking-last-confirmed-reset\"\
        <div class=\"product-tracking-x\"><time dateTime=\"2026-09-20T00:00:00.000Z\"></time></div>";
    assert_eq!(
        parse_betteropc(reordered).unwrap()["last_reset_at"],
        "2026-09-12T08:09:17.000Z"
    );
}

// ── 第四轮审计回归 ──
// 🔴-3 字节切片 panic：窗口切点落进多字节字符内部时不得 panic。
// 0..4 个 ASCII 填充把切点扫过 CJK 字符的全部字节相位——至少一个 pad 必命中老代码的 panic
#[test]
fn betteropc_cjk_at_slice_boundaries_no_panic() {
    for pad in 0..4usize {
        let x = "x".repeat(pad);
        let cjk = "重置预告更新中".repeat(30); // ~630B，落入 ±300 字符窗口
        let wall = "重置预告更新中".repeat(1200); // ~25KB，让 si+8000 字节切点必落 CJK
        let html = format!(
            "<div class=\"product-tracking-x\" data-product-id=\"codex\">{x}\
             \"dateTime\":\"2026-09-12T08:09:17.000Z\"{cjk}\
             \"className\":\"product-tracking-last-confirmed-reset\"{cjk}\
             \"dateTime\":\"2026-09-12T08:09:17.000Z\"\
             <li class=\"product-tracking-scheduled-reset\">\
             \"targetIso\":\"2026-09-22T07:00:00.000Z\",\"publishedAtIso\":\"2026-09-19T16:48:38.000Z\"\
             <span data-signal-reset-status=\"scheduled\"></span>{wall}",
        );
        let p = parse_betteropc(&html).expect("parse must not panic");
        assert_eq!(
            p["commitment"]["scheduled_for"], "2026-09-22T07:00:00.000Z",
            "pad={pad}"
        );
        assert_eq!(p["last_reset_at"], "2026-09-12T08:09:17.000Z", "pad={pad}");
    }
}

// 🟡-2/3 JS `||` falsy 语义：显式 null / "" / false / 0 都要让位给下一个候选键
#[test]
fn falsy_fields_fall_back_like_js_or() {
    // commitment: null 不得吞掉 official_signal
    let j = json!({"last_reset_at": LAST, "commitment": null,
        "official_signal": {"scheduled_for": "2026-09-25T23:00:00.000Z", "posted_at": "2026-09-19T16:48:38.000Z"}});
    assert_eq!(derive(&j, NOW_MS)["kind"], "happy");

    // 空串字段不占回退位：scheduled_for:"" 让 time 生效；posted_at:"" 让 announced_at
    let j = json!({"last_reset_at": LAST,
        "commitment": {"scheduled_for": "", "time": "2026-09-25T23:00:00.000Z",
                       "posted_at": "", "announced_at": "2026-09-19T16:48:38.000Z"}});
    let s = derive(&j, NOW_MS);
    assert_eq!(s["kind"], "happy");
    assert_eq!(s["detail"]["scheduledISO"], "2026-09-25T23:00:00.000Z");

    // commitment 整体 falsy（""/false/0）同样让位给 official_signal
    for bad in [json!(""), json!(false), json!(0)] {
        let j = json!({"last_reset_at": LAST, "commitment": bad,
            "official_signal": {"scheduled_for": "2026-09-25T23:00:00.000Z", "posted_at": "2026-09-19T16:48:38.000Z"}});
        assert_eq!(derive(&j, NOW_MS)["kind"], "happy", "commitment={bad}");
    }
}

// 🟡-4 宽松 ISO：分钟精度与显式偏移（JS Date.parse 均接受，手写时刻高频形态）
#[test]
fn loose_iso_minute_precision_and_offsets() {
    assert_eq!(ms("2026-10-01T00:00Z"), ms("2026-10-01T00:00:00.000Z"));
    assert_eq!(ms("2026-10-01T08:00+08:00"), ms("2026-10-01T00:00:00.000Z"));
    assert_eq!(ms("2026-10-01T08:00+0800"), ms("2026-10-01T00:00:00.000Z"));
    // 无偏移按宿主本地时区（对齐 Date.parse）——只断言可解析，值随机器 TZ
    assert!(parse_iso_pub("2026-10-01T00:00").is_some());
    // 裸字符串 scheduled 走同一解析器
    let j = json!({"events": [], "scheduled": "2026-09-25T23:00Z"});
    assert_eq!(derive(&j, NOW_MS)["kind"], "happy");
}

// 🟡-11 分数小时不截断：[0,23] 内的小数参与算术（22.5 → 22:30，2.5 → 02:30）
#[test]
fn fractional_time_window_hour_not_truncated() {
    let j = json!({"last_reset_at": LAST,
        "tease_signal": tease_default(json!("coming in Tuesday")),
        "time_window": {"start_hour": 22.5, "end_hour": 2.5, "timezone": "UTC"}});
    let s = derive(&j, NOW_MS);
    assert_eq!(s["detail"]["targetStart"], iso(ms("2026-09-22T22:30:00Z")));
    assert_eq!(s["detail"]["targetEnd"], iso(ms("2026-09-23T02:30:00Z")));
    // 界外分数仍被拒（两侧一致的 <=23 校验）：23.9 落回默认 23
    let j = json!({"last_reset_at": LAST,
        "tease_signal": tease_default(json!("coming in Tuesday")),
        "time_window": {"start_hour": 23.9, "end_hour": 2.5, "timezone": "UTC"}});
    assert_eq!(
        derive(&j, NOW_MS)["detail"]["targetStart"],
        iso(ms("2026-09-22T23:00:00Z"))
    );
}
