// 拉取链降级规格。运行：node --test test/
import { test, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import { fetchForecast, mirrorEnvelope, SOURCES } from '../src/net.mjs';

const PRIMARY = SOURCES.primary, RAW = SOURCES.mirrors[0], JSD = SOURCES.mirrors[1], DIRECT = SOURCES.direct, BACKUP = SOURCES.backup;
const forecast = { last_reset_at: '2026-09-19T00:00:00Z' };
const resets = { scheduled: null, events: [{ announced_at: '2026-09-19T00:00:00Z' }] };
const env = (ageMs, upstream = forecast) => ({ schema: 1, fetched_at: new Date(Date.now() - ageMs).toISOString(), source_url: DIRECT, upstream });
const BP_PAGE = '<title>Codex</title><li class="product-tracking-scheduled-reset"><span data-signal-reset-status="scheduled">已排期</span></li>'
  + '<time class="product-tracking-last-confirmed-reset" dateTime="2026-09-12T08:09:17.000Z"></time>'
  + '\\"targetIso\\":\\"2026-09-22T07:00:00.000Z\\"';

const realFetch = globalThis.fetch;
const fake = map => (url) => {
  const v = map[url];
  if (v === undefined) return Promise.reject(new Error('unexpected url ' + url));
  if (v instanceof Error) return Promise.reject(v);
  return Promise.resolve({
    ok: v.ok ?? true, status: v.status ?? 200,
    json: async () => { if (v.bad) throw new SyntaxError('not json'); return v.body; },
    text: async () => v.text ?? '',
  });
};
afterEach(() => { globalThis.fetch = realFetch; });

test('primary betteropc page → parsed upstream, via primary', async () => {
  globalThis.fetch = fake({ [PRIMARY]: { text: BP_PAGE } });
  const r = await fetchForecast('ua');
  assert.equal(r.via, 'primary');
  assert.equal(r.forecast.source, 'betteropc');
  assert.equal(r.forecast.commitment.scheduled_for, '2026-09-22T07:00:00.000Z');
  assert.equal(r.forecast.last_reset_at, '2026-09-12T08:09:17.000Z');
});

test('primary returns non-page (WAF challenge / redesign) → falls through to mirror', async () => {
  globalThis.fetch = fake({ [PRIMARY]: { text: '<html>just a moment</html>' }, [RAW]: { body: env(600e3) } });
  assert.equal((await fetchForecast('ua')).via, 'mirror');
});

test('primary down → mirror still wins before legacy sources', async () => {
  globalThis.fetch = fake({ [PRIMARY]: new Error('cf timeout'), [RAW]: { body: env(600e3) } });
  assert.equal((await fetchForecast('ua')).via, 'mirror');
});

test('fresh raw mirror wins', async () => {
  globalThis.fetch = fake({ [RAW]: { body: env(600e3) } });
  const r = await fetchForecast('ua');
  assert.equal(r.via, 'mirror'); assert.deepEqual(r.forecast, forecast);
});

test('raw down → jsdelivr mirror', async () => {
  globalThis.fetch = fake({ [RAW]: new Error('ECONNREFUSED'), [JSD]: { body: env(3600e3) } });
  assert.equal((await fetchForecast('ua')).via, 'mirror');
});

test('mirrors stale (>6h) → direct', async () => {
  globalThis.fetch = fake({ [RAW]: { body: env(7 * 3600e3) }, [JSD]: { body: env(8 * 3600e3) }, [DIRECT]: { body: forecast } });
  assert.equal((await fetchForecast('ua')).via, 'direct');
});

test('mirror whose upstream is an array or string → not fresh → skipped', async () => {
  globalThis.fetch = fake({ [RAW]: { body: env(0, []) }, [JSD]: { body: env(0, '<html>') }, [DIRECT]: { body: forecast } });
  assert.equal((await fetchForecast('ua')).via, 'direct');
});

test('mirror with wrong schema / not json / 404 → skipped', async () => {
  globalThis.fetch = fake({ [RAW]: { body: { schema: 99 } }, [JSD]: { bad: true }, [DIRECT]: { body: forecast } });
  assert.equal((await fetchForecast('ua')).via, 'direct');
  globalThis.fetch = fake({ [RAW]: { ok: false, status: 404 }, [JSD]: { ok: false, status: 404 }, [DIRECT]: { body: forecast } });
  assert.equal((await fetchForecast('ua')).via, 'direct');
});

test('direct returns HTML (site redesign) → backup source', async () => {
  globalThis.fetch = fake({ [RAW]: new Error('x'), [JSD]: new Error('y'), [DIRECT]: { bad: true }, [BACKUP]: { body: resets } });
  const r = await fetchForecast('ua');
  assert.equal(r.via, 'backup'); assert.deepEqual(r.forecast, resets);
});

test('direct 5xx → backup', async () => {
  globalThis.fetch = fake({ [RAW]: new Error('x'), [JSD]: new Error('y'), [DIRECT]: { ok: false, status: 503 }, [BACKUP]: { body: resets } });
  assert.equal((await fetchForecast('ua')).via, 'backup');
});

test('everything down → throws with all reasons', async () => {
  globalThis.fetch = fake({ [PRIMARY]: new Error('p'), [RAW]: new Error('a'), [JSD]: new Error('b'), [DIRECT]: new Error('c'), [BACKUP]: new Error('d') });
  await assert.rejects(fetchForecast('ua'), /all sources failed.*p.*a.*b.*c.*d/s);
});

test('mirror may carry backup-shaped upstream (mirror job fell back) → still accepted', async () => {
  globalThis.fetch = fake({ [RAW]: { body: env(60e3, resets) } });
  const r = await fetchForecast('ua');
  assert.equal(r.via, 'mirror'); assert.deepEqual(r.forecast, resets);
});

test('every request carries our UA', async () => {
  const seen = [];
  globalThis.fetch = (url, opts) => { seen.push(opts.headers['User-Agent']); return fake({ [RAW]: { body: env(0) } })(url); };
  await fetchForecast('is-tibo-happy/test');
  assert.ok(seen.length >= 1 && seen.every(u => u === 'is-tibo-happy/test'));
});

test('mirrorEnvelope shape', () => {
  const e = mirrorEnvelope({ a: 1 }, 'https://src');
  assert.equal(e.schema, 1); assert.equal(e.source_url, 'https://src'); assert.deepEqual(e.upstream, { a: 1 });
  assert.ok(Number.isFinite(Date.parse(e.fetched_at)));
});
