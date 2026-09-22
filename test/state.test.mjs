// 状态推导 + 文案 的规格测试。运行：node --test test/
// now 固定为 2026-09-20T12:00:00Z（周日）。时区通过 subLine 的 tz 选项显式传入，不依赖系统 TZ。
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { derive, deriveForecast, deriveState, subLine, resolveDisplay } from '../src/state.mjs';

const NOW = Date.parse('2026-09-20T12:00:00Z');   // Sunday
const H = 3600e3, D = 86400e3;
const iso = ms => new Date(ms).toISOString();
const SH = 'Asia/Shanghai', LA = 'America/Los_Angeles', UTC = 'UTC', NZ = 'Pacific/Auckland';

const LAST = '2026-09-12T08:09:17.000Z';   // 8.3 天前
const WINDOW = { start_hour: 23, end_hour: 2, label: '11 PM - 2 AM', timezone: 'UTC' };
const tease = (quote, at = '2026-09-19T16:48:38.000Z', expires = '2026-09-21T16:48:38.000Z') =>
  ({ tier: 'T1', post: { at, quote, url: 'https://x.com/x/status/1' }, expires_at: expires });

const run = (json, tz = UTC, now = NOW) => {
  const s = derive(json, now);
  return { kind: s.kind, ...subLine(s, now, { tz }) };
};

// ───────────── A. 暗示级 + 星期几 → 按 UTC 惯用窗口起点换算到用户本地星期 ─────────────
test('tease Tuesday → local weekday differs by timezone', () => {
  const j = { last_reset_at: LAST, tease_signal: tease('OK fine. But it’s also still coming in Tuesday'), time_window: WINDOW };
  // 窗口起点 2026-09-22T23:00Z：上海=周三 07:00，洛杉矶=周二 16:00，UTC=周二 23:00，奥克兰(NZST)=周三 11:00
  assert.deepEqual(run(j, SH), { kind: 'happy', zh: '预计周三重置', en: 'reset expected Wednesday' });
  assert.deepEqual(run(j, LA), { kind: 'happy', zh: '预计周二重置', en: 'reset expected Tuesday' });
  assert.deepEqual(run(j, UTC), { kind: 'happy', zh: '预计周二重置', en: 'reset expected Tuesday' });
  assert.deepEqual(run(j, NZ), { kind: 'happy', zh: '预计周三重置', en: 'reset expected Wednesday' });
});

test('tease Tuesday without time_window → default 23:00 UTC window still applies', () => {
  const j = { last_reset_at: LAST, tease_signal: tease('coming in Tuesday') };
  assert.deepEqual(run(j, SH), { kind: 'happy', zh: '预计周三重置', en: 'reset expected Wednesday' });
});

// ───────────── B. today / tomorrow 相对词：相对推文发布日（UTC），再换算本地 ─────────────
test('tease "today" posted this morning → today in UTC, tomorrow in Shanghai', () => {
  const j = { last_reset_at: LAST, tease_signal: tease('reset coming today', '2026-09-20T10:00:00Z', '2026-09-22T10:00:00Z'), time_window: WINDOW };
  // 目标窗口起点 2026-09-20T23:00Z：UTC 仍是今天；上海已是 9/21 07:00 = 明天
  assert.deepEqual(run(j, UTC), { kind: 'happy', zh: '预计今天重置', en: 'reset expected today' });
  assert.deepEqual(run(j, SH), { kind: 'happy', zh: '预计明天重置', en: 'reset expected tomorrow' });
});

test('tease "tomorrow" → post date + 1', () => {
  const j = { last_reset_at: LAST, tease_signal: tease('tomorrow for sure', '2026-09-20T10:00:00Z', '2026-09-22T10:00:00Z'), time_window: WINDOW };
  // 2026-09-21T23:00Z：UTC=周一=明天
  assert.deepEqual(run(j, UTC), { kind: 'happy', zh: '预计明天重置', en: 'reset expected tomorrow' });
  // 上海 = 9/22 07:00 = 后天 → 用星期
  assert.deepEqual(run(j, SH), { kind: 'happy', zh: '预计周二重置', en: 'reset expected Tuesday' });
});

// ───────────── B2. 相对词以 Tibo 所在时区（America/Los_Angeles）的日期为基准，不是 UTC 日 ─────────────
test('post at PT Monday evening (already Tuesday in UTC) saying "tomorrow" → PT Tuesday', () => {
  // 2026-09-22T00:30Z = PT 周一 17:30。"tomorrow" = PT 周二 → 窗口起点 2026-09-22T23:00Z
  const now = Date.parse('2026-09-22T01:00:00Z');
  const j = { last_reset_at: LAST, tease_signal: tease('tomorrow!', '2026-09-22T00:30:00Z', '2026-09-24T00:00:00Z'), time_window: WINDOW };
  assert.deepEqual(run(j, LA, now), { kind: 'happy', zh: '预计明天重置', en: 'reset expected tomorrow' });
  // UTC 用户此刻已是周二 01:00，目标窗口（周二 23:00Z）就在本地"今天"
  // （若错误地锚 UTC 日 → tomorrow 变周三 → 会显示 周三）
  assert.deepEqual(run(j, UTC, now), { kind: 'happy', zh: '预计今天重置', en: 'reset expected today' });
});

test('post at PT Tuesday evening saying "Tuesday" → that same PT Tuesday, not next week', () => {
  // 2026-09-23T00:30Z = PT 周二 17:30；窗口 2026-09-22T23:00Z → 2026-09-23T02:00Z，now 在窗口内
  const now = Date.parse('2026-09-23T01:00:00Z');
  const j = { last_reset_at: LAST, tease_signal: tease('coming in Tuesday', '2026-09-23T00:30:00Z', '2026-09-25T00:00:00Z'), time_window: WINDOW };
  // 已进入窗口 → 即将重置（任何时区）
  assert.deepEqual(run(j, LA, now), { kind: 'happy', zh: '即将重置', en: 'reset imminent' });
  assert.deepEqual(run(j, SH, now), { kind: 'happy', zh: '即将重置', en: 'reset imminent' });
});

test('tease: post beyond target window + late grace is stale → ignored', () => {
  const now = Date.parse('2026-09-25T12:00:00Z');   // 窗口端 09-23T02:00 + 36h = 09-24T14:00 已过
  const j = { last_reset_at: LAST, tease_signal: tease('coming in Tuesday', '2026-09-19T16:48:38Z', null) };
  assert.equal(run(j, UTC, now).kind, 'unhappy');
  const fresh = { last_reset_at: LAST, tease_signal: tease('coming in Friday', '2026-09-23T16:48:38Z', null) };
  assert.equal(run(fresh, UTC, now).kind, 'happy');
});

test('explicit expires_at beats the 72h default (still live at 90h if expires_at says so)', () => {
  const now = Date.parse('2026-09-23T12:00:00Z');   // 帖子 ~91h 前
  const j = { last_reset_at: LAST, tease_signal: tease('soon™', '2026-09-19T16:48:38Z', '2026-09-24T00:00:00Z') };
  assert.deepEqual(run(j, UTC, now), { kind: 'happy', zh: '有重置暗示', en: 'reset hinted' });
});

test('weekend / tonight resolve to a weekday label, never throw', () => {
  const j = w => ({ last_reset_at: LAST, tease_signal: tease(w, '2026-09-20T10:00:00Z', '2026-09-27T00:00:00Z'), time_window: WINDOW });
  // 2026-09-20 是周日；"weekend" → 下一个周六 09-26 起点 23:00Z → UTC 周六
  assert.deepEqual(run(j('this weekend'), UTC), { kind: 'happy', zh: '预计周六重置', en: 'reset expected Saturday' });
  // tonight = today（PT 周日）→ 窗口 09-20T23:00Z → UTC 今天
  assert.deepEqual(run(j('tonight probably'), UTC), { kind: 'happy', zh: '预计今天重置', en: 'reset expected today' });
});

test('invalid tz string degrades to hinted instead of throwing', () => {
  const j = { last_reset_at: LAST, tease_signal: tease('coming in Tuesday'), time_window: WINDOW };
  assert.deepEqual(run(j, 'Mars/Olympus'), { kind: 'happy', zh: '有重置暗示', en: 'reset hinted' });
});

test('detail carries no volatile timestamp (dedup must work across polls)', () => {
  const j = { last_reset_at: LAST, tease_signal: tease('coming in Tuesday'), time_window: WINDOW };
  const a = derive(j, NOW), b = derive(j, NOW + 60e3);
  assert.deepEqual(a, b);
});

// ───────────── C/D. 暗示过期 / 窗口已过但暗示仍在 ─────────────
test('expired tease is ignored only after late grace ends → ledger decides', () => {
  const now = Date.parse('2026-09-24T15:00:00Z');   // 目标窗口端 09-23T02:00 + 36h 已过
  const j = { last_reset_at: LAST, tease_signal: tease('coming in Tuesday'), time_window: WINDOW };
  assert.deepEqual(run(j, UTC, now), { kind: 'unhappy', zh: '暂无重置预告', en: 'no reset news' });
});

test('tease live but its window already passed → "due" copy (late, still valid)', () => {
  const now = Date.parse('2026-09-23T12:00:00Z');   // Wed noon; window ended Wed 02:00Z, grace alive
  const j = { last_reset_at: LAST, tease_signal: tease('coming in Tuesday', '2026-09-19T16:48:38Z', '2026-09-24T00:00:00Z'), time_window: WINDOW };
  assert.deepEqual(run(j, UTC, now), { kind: 'happy', zh: '随时重置', en: 'reset any time now' });
});

test('tease with unparseable quote → hinted', () => {
  const j = { last_reset_at: LAST, tease_signal: tease('soon™') };
  assert.deepEqual(run(j), { kind: 'happy', zh: '有重置暗示', en: 'reset hinted' });
});

test('expires_at 已过只在无其他存活理由时才判死（上游提前撤字段不闪断）', () => {
  // 帖子仍新鲜（<72h）：expires_at 刚过也维持 HAPPY
  const live = { last_reset_at: LAST, tease_signal: tease('coming in Tuesday', '2026-09-19T16:48:38Z', iso(NOW)) };
  assert.equal(run(live).kind, 'happy');
  // 帖子老、quote 无目标、expires 已过 → 真过期
  const dead = { last_reset_at: LAST, tease_signal: tease('soon™', '2026-09-15T16:48:38Z', iso(NOW - H)) };
  assert.equal(run(dead).kind, 'unhappy');
});

// ───────────── I. 弱信号兜底 latest_hint + 迟到宽限 + 兑现让位 ─────────────
// 产品规则：官方预告极少跳票，只会早来或迟到。上游撤字段不闪断；落地后让位账本。
test('upstream dropped tease_signal but latest_hint still fresh → hedged happy (2026-09-22 incident)', () => {
  const j = {
    last_reset_at: LAST,
    latest_hint: { id: 'x', at: '2026-09-19T16:48:38.000Z', url: 'https://x.com/x/1', quote: 'OK fine. But it’s also still coming in Tuesday', day: null },
    time_window: WINDOW,
  };
  const now = Date.parse('2026-09-22T02:00:00Z');   // 目标窗口 09-22T23:00Z 未到
  assert.deepEqual(run(j, UTC, now), { kind: 'happy', zh: '预计今天重置', en: 'reset expected today' });
  assert.deepEqual(run(j, SH, now), { kind: 'happy', zh: '预计明天重置', en: 'reset expected tomorrow' });
});

test('hint target window passed but within late grace → happy "due"', () => {
  const j = { last_reset_at: LAST, latest_hint: { at: '2026-09-19T16:48:38.000Z', quote: 'coming in Tuesday' }, time_window: WINDOW };
  const now = Date.parse('2026-09-23T12:00:00Z');   // 窗口结束 10h，仍在 36h 宽限内
  assert.deepEqual(run(j, UTC, now), { kind: 'happy', zh: '随时重置', en: 'reset any time now' });
});

test('hint fully stale (window end + 36h passed) → unhappy', () => {
  const j = { last_reset_at: LAST, latest_hint: { at: '2026-09-19T16:48:38.000Z', quote: 'coming in Tuesday' }, time_window: WINDOW };
  const now = Date.parse('2026-09-24T15:00:00Z');
  assert.deepEqual(run(j, UTC, now), { kind: 'unhappy', zh: '暂无重置预告', en: 'no reset news' });
});

test('latest_hint with unparseable quote is not a signal', () => {
  const j = { last_reset_at: LAST, latest_hint: { at: '2026-09-19T16:48:38.000Z', quote: 'hmm interesting' } };
  assert.equal(run(j).kind, 'unhappy');
});

test('latest_hint older than 7d is ignored entirely', () => {
  const j = { last_reset_at: LAST, latest_hint: { at: '2026-09-10T16:48:38.000Z', quote: 'coming in Tuesday' } };
  assert.equal(run(j).kind, 'unhappy');
});

test('reset landed after the tease was posted → fulfilled → 刚刚重置', () => {
  const j = {
    last_reset_at: '2026-09-22T23:30:00.000Z',   // 重置已在周二窗口内落地
    tease_signal: tease('coming in Tuesday', '2026-09-19T16:48:38Z', '2026-09-25T00:00:00Z'),
    time_window: WINDOW,
  };
  const now = Date.parse('2026-09-23T01:00:00Z');
  assert.deepEqual(run(j, UTC, now), { kind: 'happy', zh: '刚刚重置', en: 'just reset' });
});

test('tease posted AFTER last reset → live signal, not fulfilled', () => {
  const j = { last_reset_at: '2026-09-18T08:00:00Z', tease_signal: tease('coming in Tuesday'), time_window: WINDOW };
  assert.deepEqual(run(j, LA), { kind: 'happy', zh: '预计周二重置', en: 'reset expected Tuesday' });
});

test('resets shape: scheduled_for overdue within grace → due; fulfilled → ledger', () => {
  const j = { scheduled: { scheduled_for: iso(NOW - 8 * H), display_text: 'x', announced_at: iso(NOW - 2 * D) }, events: [{ announced_at: LAST }] };
  assert.deepEqual(run(j), { kind: 'happy', zh: '随时重置', en: 'reset any time now' });
  const done = { scheduled: { scheduled_for: '2026-09-19T23:00:00Z', display_text: 'x', announced_at: '2026-09-18T00:00:00Z' }, events: [{ announced_at: '2026-09-20T01:00:00Z' }] };
  assert.deepEqual(run(done), { kind: 'happy', zh: '刚刚重置', en: 'just reset' });
});

// ───────────── E. 明确承诺 → 倒计时 ─────────────
test('commitment countdown granularity', () => {
  const c = t => ({ last_reset_at: LAST, commitment: { scheduled_for: iso(NOW + t) } });
  assert.deepEqual(run(c(6.2 * D)), { kind: 'happy', zh: '6 天后重置', en: 'reset in 6 days' });
  assert.deepEqual(run(c(1.4 * D)), { kind: 'happy', zh: '34 小时后重置', en: 'reset in ~34h' });
  assert.deepEqual(run(c(3 * H)), { kind: 'happy', zh: '3 小时后重置', en: 'reset in ~3h' });
  assert.deepEqual(run(c(30 * 60e3)), { kind: 'happy', zh: '即将重置', en: 'reset imminent' });
  assert.deepEqual(run(c(1 * D)), { kind: 'happy', zh: '24 小时后重置', en: 'reset in ~24h' });
  assert.deepEqual(run(c(2 * D)), { kind: 'happy', zh: '2 天后重置', en: 'reset in 2 days' });
  assert.deepEqual(run(c(1.5 * D)), { kind: 'happy', zh: '36 小时后重置', en: 'reset in ~36h' });
});

test('commitment time just passed (<6h) → imminent; long passed → due (late)', () => {
  const c = t => ({ last_reset_at: LAST, commitment: { scheduled_for: iso(NOW + t) } });
  assert.deepEqual(run(c(-60e3)), { kind: 'happy', zh: '即将重置', en: 'reset imminent' });
  assert.deepEqual(run(c(-7 * H)), { kind: 'happy', zh: '随时重置', en: 'reset any time now' });
});

test('commitment overdue beyond late grace → stale, falls to ledger', () => {
  const j = { last_reset_at: LAST, commitment: { scheduled_for: iso(NOW - 2 * D) } };
  assert.deepEqual(run(j), { kind: 'unhappy', zh: '暂无重置预告', en: 'no reset news' });
});

test('commitment beats tease when both present', () => {
  const j = { last_reset_at: LAST, commitment: { scheduled_for: iso(NOW + 5 * D) }, tease_signal: tease('coming in Tuesday') };
  assert.deepEqual(run(j), { kind: 'happy', zh: '5 天后重置', en: 'reset in 5 days' });
});

// ───────────── E2. 承诺级信号的锚定规则（审计回归：无锚不永生、不滚动） ─────────────
test('commitment without any parseable time anchor is not a signal (no immortal happy)', () => {
  const j = { last_reset_at: LAST, commitment: { text: 'reset coming eventually' } };
  assert.equal(run(j).kind, 'unhappy');
  assert.equal(run(j, UTC, NOW + 90 * D).kind, 'unhappy');   // 90 天后也不复活
});

test('official_signal text without post time does not re-anchor "next Tuesday" to now', () => {
  const j = { last_reset_at: LAST, official_signal: { text: 'by Tuesday' } };
  assert.equal(run(j).kind, 'unhappy');
  assert.equal(run(j, UTC, NOW + 90 * D).kind, 'unhappy');   // 无滚动锚：三个月后同样 unhappy
});

test('official_signal with real posted_at anchors the weekday to the post date', () => {
  const j = { last_reset_at: LAST, official_signal: { text: 'by Tuesday', posted_at: '2026-09-19T16:48:38.000Z' }, time_window: WINDOW };
  assert.deepEqual(run(j, LA), { kind: 'happy', zh: '预计周二重置', en: 'reset expected Tuesday' });
  // 发布超过 7 天 → 信号过期
  const old = { last_reset_at: LAST, official_signal: { text: 'by Tuesday', posted_at: '2026-09-10T16:48:38.000Z' } };
  assert.equal(run(old).kind, 'unhappy');
});

test('non-string quote / display_text never throws', () => {
  // 上游字段形状异常时 derive 不得抛错——否则守护进程整次拉取判失败，好账本 12h 后退化 OFFLINE
  const j = { last_reset_at: LAST, tease_signal: tease(42) };
  assert.deepEqual(run(j), { kind: 'happy', zh: '有重置暗示', en: 'reset hinted' });   // quote 解不出 → hinted（帖子仍新鲜）
  const hint = { last_reset_at: LAST, latest_hint: { at: '2026-09-19T16:48:38.000Z', quote: { text: 'x' } } };
  assert.equal(run(hint).kind, 'unhappy');   // hint 必须解出目标才算信号
  const r = { scheduled: { display_text: { text: 'soon' }, announced_at: '2026-09-19T16:48:38.000Z' }, events: [{ announced_at: LAST }] };
  assert.equal(run(r).kind, 'unhappy');
});

// ───────────── F. 窗口制 → 最晚边，本地星期 ─────────────
test('teased_window end → "by <local weekday of end>"', () => {
  const j = { last_reset_at: LAST, official_signal: { text: 'by Tuesday' }, teased_window: { start: '2026-09-22T23:00:00Z', end: '2026-09-23T02:00:00Z' } };
  assert.deepEqual(run(j, LA), { kind: 'happy', zh: '最晚周二重置', en: 'reset by Tuesday' });
  assert.deepEqual(run(j, SH), { kind: 'happy', zh: '最晚周三重置', en: 'reset by Wednesday' });
});

test('teased_window alone still counts as a signal; overdue within grace → due', () => {
  const j = { last_reset_at: LAST, teased_window: { start: '2026-09-22T23:00:00Z', end: '2026-09-23T02:00:00Z' } };
  assert.deepEqual(run(j, LA), { kind: 'happy', zh: '最晚周二重置', en: 'reset by Tuesday' });
  // 窗口刚过、仍在迟到宽限 → HAPPY "随时重置"
  const late = Date.parse('2026-09-24T12:00:00Z');
  assert.deepEqual(run(j, LA, late), { kind: 'happy', zh: '随时重置', en: 'reset any time now' });
  // 宽限也过了 → 账本
  const dead = Date.parse('2026-09-25T12:00:00Z');
  assert.equal(run(j, LA, dead).kind, 'unhappy');
});

test('teased_window with start > end (malformed) → window dropped; anchorless text is not a signal', () => {
  const j = { last_reset_at: LAST, official_signal: { text: 'by Tuesday' }, teased_window: { start: '2026-09-23T02:00:00Z', end: '2026-09-22T23:00:00Z' } };
  // 窗口非法被丢弃后，'by Tuesday' 没有可锚定的发布时间 → 不算信号
  // （若拿 now 当发布日会"滚动重锚"：每次推导都显示新鲜的"预计周二重置"，永不失效）
  assert.deepEqual(run(j, UTC), { kind: 'unhappy', zh: '暂无重置预告', en: 'no reset news' });
});

// ───────────── G. 账本 ─────────────
test('ledger: recent resets stay HAPPY, >3d → UNHAPPY with no day count', () => {
  const l = d => ({ last_reset_at: iso(NOW - d * D) });
  assert.deepEqual(run(l(0.5)), { kind: 'happy', zh: '刚刚重置', en: 'just reset' });
  assert.deepEqual(run(l(2.1)), { kind: 'happy', zh: '上次重置 2 天前', en: 'last reset 2d ago' });
  assert.deepEqual(run(l(3.0)), { kind: 'happy', zh: '上次重置 3 天前', en: 'last reset 3d ago' });
  assert.deepEqual(run(l(3.01)), { kind: 'unhappy', zh: '暂无重置预告', en: 'no reset news' });
  assert.deepEqual(run(l(30)), { kind: 'unhappy', zh: '暂无重置预告', en: 'no reset news' });
});

test('ledger: last_reset_at in the future (clock skew) → treated as just reset', () => {
  assert.deepEqual(run({ last_reset_at: iso(NOW + D) }), { kind: 'happy', zh: '刚刚重置', en: 'just reset' });
});

// ───────────── H. 垃圾输入 ─────────────
test('garbage upstream → offline (not an object) or unhappy (object but underivable), never throws', () => {
  assert.equal(derive(null, NOW).kind, 'offline');
  assert.equal(derive(undefined, NOW).kind, 'offline');
  assert.equal(derive('<html>cloudflare</html>', NOW).kind, 'offline');
  assert.equal(derive([], NOW).kind, 'offline');
  assert.equal(derive(42, NOW).kind, 'offline');
  assert.deepEqual(run({}), { kind: 'unhappy', zh: '暂无重置预告', en: 'no reset news' });
  assert.deepEqual(run({ last_reset_at: 'garbage' }), { kind: 'unhappy', zh: '暂无重置预告', en: 'no reset news' });
  assert.equal(run({ age_days: '8' }).kind, 'unhappy');
  assert.equal(run({ last_reset_at: LAST, tease_signal: 'not-an-object' }).kind, 'unhappy');
  assert.equal(run({ last_reset_at: LAST, tease_signal: { post: null } }).kind, 'unhappy');
  assert.equal(run({ last_reset_at: LAST, commitment: 'yes' }).kind, 'unhappy');   // 无时间锚的 commitment 不算信号
  assert.equal(run({ last_reset_at: LAST, time_window: { start_hour: 'x' }, tease_signal: tease('coming in Tuesday') }, SH).zh, '预计周三重置'); // bad window → default
  assert.equal(subLine({ kind: 'offline', detail: {} }, NOW).zh, '数据不可用');
});

// ───────────── K/L. 备用源 codex-resets.com/api/resets 形状 ─────────────
test('resets shape: scheduled tease → happy with localized weekday', () => {
  const j = {
    scheduled: { display_text: '@udi OK fine. But it’s also still coming in Tuesday', announced_at: '2026-09-19T16:48:38.000Z', scheduled_for: null, reset_type: 'banked' },
    events: [{ announced_at: LAST, reset_type: 'regular' }],
  };
  assert.deepEqual(run(j, LA), { kind: 'happy', zh: '预计周二重置', en: 'reset expected Tuesday' });
  assert.deepEqual(run(j, SH), { kind: 'happy', zh: '预计周三重置', en: 'reset expected Wednesday' });
});

test('resets shape: scheduled_for exact → countdown', () => {
  const j = { scheduled: { scheduled_for: iso(NOW + 4 * D), display_text: 'x' }, events: [] };
  assert.deepEqual(run(j), { kind: 'happy', zh: '4 天后重置', en: 'reset in 4 days' });
});

test('resets shape: bare-string scheduled must parse as a date within grace', () => {
  assert.deepEqual(run({ scheduled: '2026-09-25T23:00:00.000Z', events: [{ announced_at: LAST }] }),
    { kind: 'happy', zh: '5 天后重置', en: 'reset in 5 days' });
  assert.equal(run({ scheduled: 'coming Tuesday', events: [{ announced_at: LAST }] }).kind, 'unhappy');
  assert.equal(run({ scheduled: '2026-09-15T23:00:00.000Z', events: [{ announced_at: LAST }] }).kind, 'unhappy');
});

test('resets shape: stale scheduled (announced >48h ago, no scheduled_for) is ignored', () => {
  const now = Date.parse('2026-09-25T12:00:00Z');
  const j = { scheduled: { display_text: 'coming in Tuesday', announced_at: '2026-09-19T16:48:38.000Z', scheduled_for: null }, events: [{ announced_at: LAST }] };
  assert.deepEqual(run(j, UTC, now), { kind: 'unhappy', zh: '暂无重置预告', en: 'no reset news' });
});

test('resets shape: ledger rules match forecast shape', () => {
  assert.deepEqual(run({ events: [{ announced_at: iso(NOW - 1 * D) }] }), { kind: 'happy', zh: '上次重置 1 天前', en: 'last reset 1d ago' });
  assert.deepEqual(run({ events: [{ announced_at: LAST }] }), { kind: 'unhappy', zh: '暂无重置预告', en: 'no reset news' });
  assert.deepEqual(run({ events: [] }), { kind: 'unhappy', zh: '暂无重置预告', en: 'no reset news' });
  assert.deepEqual(run({ events: [{ announced_at: 'bad' }] }), { kind: 'unhappy', zh: '暂无重置预告', en: 'no reset news' });
});

test('derive dispatches on shape; direct exports still work', () => {
  assert.equal(deriveForecast({ last_reset_at: LAST }, NOW).kind, 'unhappy');
  assert.equal(deriveState({ events: [{ announced_at: LAST }] }, NOW).kind, 'unhappy');
  assert.equal(derive({ events: [] }, NOW).kind, 'unhappy');
  assert.equal(derive({ last_reset_at: LAST }, NOW).kind, 'unhappy');
});

// ───────────── subLine 兜底：detail.sub 是 Node 产物，widget 也可能拿到旧格式 ─────────────
test('subLine never throws on partial detail', () => {
  for (const kind of ['happy', 'unhappy', 'offline']) {
    const o = subLine({ kind, detail: {} }, NOW);
    assert.equal(typeof o.zh, 'string'); assert.equal(typeof o.en, 'string');
    assert.ok(o.zh.length > 0 && o.en.length > 0);
  }
});

// ───────────── 守护进程展示决策：新数据 > 12h 内缓存 > OFFLINE ─────────────
test('resolveDisplay: fresh result wins and becomes the new cache', () => {
  const fresh = { kind: 'happy', detail: { sub: { zh: 'a', en: 'a' } } };
  const r = resolveDisplay(fresh, { state: { kind: 'unhappy', detail: {} }, at: NOW - H }, NOW);
  assert.deepEqual(r.state, fresh);
  assert.deepEqual(r.cache, { state: fresh, at: NOW });
});

test('resolveDisplay: fetch failed but cache < 12h → keep showing cached state unchanged', () => {
  const cached = { kind: 'happy', detail: { sub: { zh: '预计周三重置', en: 'x' } } };
  const r = resolveDisplay(null, { state: cached, at: NOW - 11.9 * H }, NOW);
  assert.deepEqual(r.state, cached);
  assert.deepEqual(r.cache, { state: cached, at: NOW - 11.9 * H });   // cache 不被刷新
});

test('resolveDisplay: fetch failed and cache ≥ 12h (or absent) → OFFLINE', () => {
  const cached = { kind: 'happy', detail: {} };
  assert.equal(resolveDisplay(null, { state: cached, at: NOW - 12 * H }, NOW).state.kind, 'offline');
  assert.equal(resolveDisplay(null, null, NOW).state.kind, 'offline');
  assert.equal(resolveDisplay(null, { state: cached, at: 'garbage' }, NOW).state.kind, 'offline');
  assert.equal(resolveDisplay(null, { state: 'nope', at: NOW }, NOW).state.kind, 'offline');
  // OFFLINE 态自带双语副行
  const o = resolveDisplay(null, null, NOW).state;
  assert.deepEqual(o.detail.sub, { zh: '数据不可用', en: 'data unavailable' });
});

test('resolveDisplay: a cached offline is never "kept" — expired cache does not resurrect', () => {
  // 缓存只存好状态；即便传入 offline 也不当作可沿用的数据
  const r = resolveDisplay(null, { state: { kind: 'offline', detail: {} }, at: NOW }, NOW);
  assert.equal(r.state.kind, 'offline');
});

test('resolveDisplay: at 数字字符串/NaN/缺失、数组缓存 → OFFLINE（严格拒收）', () => {
  const good = { kind: 'happy', detail: { sub: { zh: 'x', en: 'x' } } };
  assert.equal(resolveDisplay(null, { state: good, at: String(NOW - H) }, NOW).state.kind, 'offline');
  assert.equal(resolveDisplay(null, { state: good, at: NaN }, NOW).state.kind, 'offline');
  assert.equal(resolveDisplay(null, { state: good }, NOW).state.kind, 'offline');
  assert.equal(resolveDisplay(null, [1, 2], NOW).state.kind, 'offline');
});

test('resolveDisplay: at 在未来不无限续命（容 1h 前向偏差）', () => {
  const good = { kind: 'happy', detail: { sub: { zh: 'x', en: 'x' } } };
  assert.equal(resolveDisplay(null, { state: good, at: NOW + 30 * 60e3 }, NOW).state.kind, 'happy');
  assert.equal(resolveDisplay(null, { state: good, at: NOW + 72 * H }, NOW).state.kind, 'offline');
});

test('resolveDisplay: 旧形状 kind（waiting）/缺 detail 的缓存不沿用', () => {
  assert.equal(resolveDisplay(null, { state: { kind: 'waiting', detail: { sub: { zh: 'x', en: 'x' } } }, at: NOW - H }, NOW).state.kind, 'offline');
  assert.equal(resolveDisplay(null, { state: { kind: 'happy' }, at: NOW - H }, NOW).state.kind, 'offline');
});

test('resolveDisplay: offline-kind fresh 视为没拿到数据，不覆盖好缓存', () => {
  const good = { kind: 'happy', detail: { sub: { zh: 'x', en: 'x' } } };
  const r = resolveDisplay({ kind: 'offline', detail: {} }, { state: good, at: NOW - H }, NOW);
  assert.equal(r.state.kind, 'happy');
  assert.deepEqual(r.cache.state, good);
});

test('resolveDisplay: cached state is re-worded for "now" (relative words must not go stale)', () => {
  // 缓存写于周一 20:00 UTC（窗口前），说"预计今天重置"；周二 06:00 回放（10h，缓存有效）→ 窗口 02:00 已过、暗示仍在 → 有重置暗示
  const written = Date.parse('2026-09-21T20:00:00Z');
  const s = derive({ last_reset_at: LAST, tease_signal: tease('coming today', '2026-09-21T10:00:00Z', '2026-09-23T00:00:00Z'), time_window: WINDOW }, written);
  s.detail.sub = subLine(s, written, { tz: UTC });
  assert.equal(s.detail.sub.zh, '预计今天重置');
  const later = Date.parse('2026-09-22T06:00:00Z');
  const r = resolveDisplay(null, { state: s, at: written }, later, { tz: UTC });
  assert.equal(r.state.kind, 'happy');
  assert.equal(r.state.detail.sub.zh, '随时重置');   // 窗口刚过 → 迟到中
  // 账龄型也要随时间走：缓存时 2.5 天，10 小时后仍 2 天；再过 1 天 → 3.9 天 → 仍沿用缓存但副行不撒谎
  const l = derive({ last_reset_at: iso(NOW - 2.5 * D) }, NOW); l.detail.sub = subLine(l, NOW);
  assert.equal(resolveDisplay(null, { state: l, at: NOW }, NOW + 10 * H).state.detail.sub.zh, '上次重置 2 天前');
});
