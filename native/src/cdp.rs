// Minimal CDP client: HTTP /json 端点 + 阻塞式 WebSocket（零 async）。
// 结构：一个 ws 线程独占 socket——500ms 读超时轮询，间隙排空出站队列；
// send() 可任意线程调用（进 pending map → 等回包）；事件经 channel 交给会话循环。
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;
use tungstenite::{client, Message};

pub const PORT: u16 = 9333;

pub fn list_targets(port: u16) -> Result<Vec<Value>, String> {
    let r = ureq::get(&format!("http://127.0.0.1:{port}/json"))
        .timeout(Duration::from_secs(3))
        .call()
        .map_err(|e| format!("/json -> {e}"))?;
    r.into_json::<Vec<Value>>().map_err(|e| format!("/json parse: {e}"))
}

pub fn get_version(port: u16) -> Result<Value, String> {
    let r = ureq::get(&format!("http://127.0.0.1:{port}/json/version"))
        .timeout(Duration::from_secs(3))
        .call()
        .map_err(|e| format!("/json/version -> {e}"))?;
    r.into_json::<Value>().map_err(|e| format!("/json/version parse: {e}"))
}

pub enum CdpEvent {
    Message { method: String, params: Value },
    Closed,
}

pub struct Cdp {
    outbound: mpsc::Sender<String>,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value, String>>>>>,
    events: Mutex<mpsc::Receiver<CdpEvent>>,
    next_id: AtomicU64,
    closed: Arc<AtomicBool>,
}

impl Cdp {
    pub fn connect(ws_url: &str, timeout: Duration) -> Result<Cdp, String> {
        // ws://host:port/path —— 本进程只会连 127.0.0.1，手工解析足够
        let hp = ws_url
            .strip_prefix("ws://")
            .ok_or("ws url must start with ws://")?;
        let authority = hp.split('/').next().unwrap_or("");
        let addr = authority
            .to_socket_addrs()
            .map_err(|e| format!("ws addr: {e}"))?
            .next()
            .ok_or("ws addr: empty")?;
        let stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("ws connect: {e}"))?;
        stream.set_read_timeout(Some(timeout)).ok();
        let req = ws_url.parse::<tungstenite::http::Uri>().map_err(|e| format!("ws uri: {e}"))?;
        let (mut ws, _) = client(req, stream).map_err(|e| format!("ws handshake: {e}"))?;
        // 握手完成后降到 500ms 轮询读，好让出站帧插进来
        ws.get_ref().set_read_timeout(Some(Duration::from_millis(500))).ok();

        let pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value, String>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (out_tx, out_rx) = mpsc::channel::<String>();
        let (ev_tx, ev_rx) = mpsc::channel::<CdpEvent>();
        let closed = Arc::new(AtomicBool::new(false));

        {
            let pending = Arc::clone(&pending);
            let closed = Arc::clone(&closed);
            thread::spawn(move || {
                loop {
                    match ws.read() {
                        Ok(Message::Text(t)) => {
                            if let Ok(msg) = serde_json::from_str::<Value>(&t) {
                                if let Some(id) = msg.get("id").and_then(|i| i.as_u64()) {
                                    let tx = pending.lock().unwrap().remove(&id);
                                    if let Some(tx) = tx {
                                        let r = if let Some(err) = msg.get("error") {
                                            Err(format!("{}: {}", err["code"], err["message"]))
                                        } else {
                                            Ok(msg.get("result").cloned().unwrap_or(Value::Null))
                                        };
                                        let _ = tx.send(r);
                                    }
                                } else if let Some(m) = msg.get("method").and_then(|m| m.as_str()) {
                                    let _ = ev_tx.send(CdpEvent::Message {
                                        method: m.to_string(),
                                        params: msg.get("params").cloned().unwrap_or(Value::Null),
                                    });
                                }
                            }
                        }
                        Ok(Message::Close(_)) => break,
                        Ok(_) => {}
                        Err(tungstenite::Error::Io(e))
                            if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
                        Err(_) => break,
                    }
                    while let Ok(frame) = out_rx.try_recv() {
                        if ws.send(Message::Text(frame.into())).is_err() {
                            break;
                        }
                    }
                    if closed.load(Ordering::Relaxed) {
                        break;
                    }
                }
                closed.store(true, Ordering::Relaxed);
                for (_, tx) in pending.lock().unwrap().drain() {
                    let _ = tx.send(Err("ws closed".into()));
                }
                let _ = ev_tx.send(CdpEvent::Closed);
            });
        }

        Ok(Cdp { outbound: out_tx, pending, events: Mutex::new(ev_rx), next_id: AtomicU64::new(0), closed })
    }

    pub fn send(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, tx);
        if self
            .outbound
            .send(json!({"id": id, "method": method, "params": params}).to_string())
            .is_err()
        {
            self.pending.lock().unwrap().remove(&id); // 发不出去就别留尸
            return Err("ws outbound closed".to_string());
        }
        match rx.recv_timeout(timeout) {
            Ok(r) => r,
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("cdp send timeout: {method}"))
            }
        }
    }

    pub fn recv_event(&self, timeout: Duration) -> Result<CdpEvent, mpsc::RecvTimeoutError> {
        self.events.lock().unwrap().recv_timeout(timeout)
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}

// Evaluate an expression in a target, return the value (throwing on exception).
pub fn eval_js(conn: &Cdp, expression: &str) -> Result<Value, String> {
    let r = conn.send(
        "Runtime.evaluate",
        json!({
            "expression": format!("{{\n{expression}\n}}"),
            "awaitPromise": false,
            "returnByValue": true,
        }),
        Duration::from_secs(10),
    )?;
    if let Some(e) = r.get("exceptionDetails") {
        let desc = e
            .get("exception")
            .and_then(|x| x.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or("");
        return Err(format!("page exception: {} {}", e["text"], desc));
    }
    Ok(r.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(Value::Null))
}
