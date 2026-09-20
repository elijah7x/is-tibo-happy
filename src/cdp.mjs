// Minimal CDP client using Node 24 native fetch + global WebSocket. Zero deps.

export async function listTargets(port = 9333) {
  const res = await fetch(`http://127.0.0.1:${port}/json`, { signal: AbortSignal.timeout(3000) });
  if (!res.ok) throw new Error(`/json -> ${res.status}`);
  return res.json();
}

export async function getVersion(port = 9333) {
  const res = await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(3000) });
  if (!res.ok) throw new Error(`/json/version -> ${res.status}`);
  return res.json();
}

export function connect(wsUrl, { timeoutMs = 15000 } = {}) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(wsUrl);
    let id = 0;
    const pending = new Map();
    const listeners = new Map();
    const closeCbs = [];
    const timer = setTimeout(() => {
      try { ws.close(); } catch {}
      reject(new Error('ws connect timeout'));
    }, timeoutMs);

    ws.addEventListener('open', () => {
      clearTimeout(timer);
      resolve({
        send(method, params = {}, timeoutMs = 10000) {
          return new Promise((res2, rej2) => {
            const msgId = ++id;
            const t = setTimeout(() => {
              pending.delete(msgId);
              rej2(new Error(`cdp send timeout: ${method}`));
            }, timeoutMs);
            pending.set(msgId, {
              res2: v => { clearTimeout(t); res2(v); },
              rej2: e => { clearTimeout(t); rej2(e); },
            });
            ws.send(JSON.stringify({ id: msgId, method, params }));
          });
        },
        on(event, cb) {
          if (!listeners.has(event)) listeners.set(event, []);
          listeners.get(event).push(cb);
        },
        onClose(cb) { closeCbs.push(cb); },
        close() { try { ws.close(); } catch {} },
      });
    });
    ws.addEventListener('close', () => {
      for (const { rej2 } of pending.values()) rej2(new Error('ws closed'));
      pending.clear();
      for (const cb of closeCbs) cb();
    });
    ws.addEventListener('error', (e) => {
      clearTimeout(timer);
      reject(new Error('ws error: ' + (e.message || 'unknown')));
    });
    ws.addEventListener('message', (ev) => {
      let msg;
      try { msg = JSON.parse(ev.data); } catch { return; }
      if (msg.id != null && pending.has(msg.id)) {
        const { res2, rej2 } = pending.get(msg.id);
        pending.delete(msg.id);
        if (msg.error) rej2(new Error(`${msg.error.code}: ${msg.error.message}`));
        else res2(msg.result);
      } else if (msg.method) {
        for (const cb of listeners.get(msg.method) || []) cb(msg.params);
      }
    });
  });
}

// Evaluate an expression in a target, return the value (throwing on exception).
export async function evalJs(conn, expression, { awaitPromise = false } = {}) {
  const r = await conn.send('Runtime.evaluate', {
    expression: '{\n' + expression + '\n}',
    awaitPromise,
    returnByValue: true,
  });
  if (r.exceptionDetails) {
    const e = r.exceptionDetails;
    throw new Error(`page exception: ${e.text} ${e.exception?.description || ''}`);
  }
  return r.result?.value;
}
