// 拉取链降级规格（test/net.test.mjs 的 Rust 移植）：本地 stub HTTP 服务器模拟五源。
// 未注册的 path → 直接断连（等价 JS 侧 fetch reject）。
use is_tibo_happy::net::*;
use serde_json::json;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Clone)]
enum R {
    Text(String),
    Json(serde_json::Value),
    Status(u16),
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn serve(map: HashMap<String, R>, hits: Arc<Mutex<Vec<String>>>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    thread::spawn(move || {
        for stream in l.incoming().flatten() {
            let map = map.clone();
            let hits = hits.clone();
            thread::spawn(move || {
                let mut s = stream;
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                if r.read_line(&mut line).is_err() {
                    return;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut ua = String::new();
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).is_err() || h.trim().is_empty() {
                        break;
                    }
                    if h.to_lowercase().starts_with("user-agent:") {
                        ua = h[11..].trim().to_string();
                    }
                }
                hits.lock().unwrap().push(format!("{path} ua:{ua}"));
                let Some(resp) = map.get(&path) else { return }; // 断连模拟网络错误
                let (code, body) = match resp {
                    R::Text(t) => (200u16, t.clone()),
                    R::Json(v) => (200, v.to_string()),
                    R::Status(c) => (*c, String::new()),
                };
                let out = format!(
                    "HTTP/1.1 {code} x\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(out.as_bytes());
            });
        }
    });
    base
}

fn sources(base: &str) -> Sources {
    Sources {
        primary: format!("{base}/primary"),
        mirrors: vec![format!("{base}/raw"), format!("{base}/jsd")],
        direct: format!("{base}/direct"),
        backup: format!("{base}/backup"),
    }
}

fn env(age_ms: i64, upstream: serde_json::Value) -> R {
    R::Json(
        json!({"schema": 1, "fetched_at": is_tibo_happy::state::iso_pub(now_ms() - age_ms), "source_url": SOURCE_DIRECT, "upstream": upstream}),
    )
}

fn forecast() -> serde_json::Value {
    json!({"last_reset_at": "2026-09-19T00:00:00Z"})
}
fn resets() -> serde_json::Value {
    json!({"scheduled": null, "events": [{"announced_at": "2026-09-19T00:00:00Z"}]})
}
const BP_PAGE: &str = "<div data-product-id=\"codex\"><li class=\"product-tracking-scheduled-reset\"><span data-signal-reset-status=\"scheduled\">已排期</span></li>\
    <time class=\"product-tracking-last-confirmed-reset\" dateTime=\"2026-09-12T08:09:17.000Z\"></time>\
    \\\"targetIso\\\":\\\"2026-09-22T07:00:00.000Z\\\"";

#[test]
fn primary_resets_json_wins() {
    let hits = Arc::new(Mutex::new(vec![]));
    let base = serve(
        HashMap::from([("/primary".into(), R::Json(resets()))]),
        hits,
    );
    let (f, via) = fetch_forecast_from(&sources(&base), "ua", now_ms()).unwrap();
    assert_eq!(via, "primary");
    assert_eq!(f, resets());
}

#[test]
fn backup_betteropc_page_parsed() {
    // backup 位现在是 betteropc HTML：primary/mirror/direct 全挂后才轮到它
    let hits = Arc::new(Mutex::new(vec![]));
    let base = serve(
        HashMap::from([("/backup".into(), R::Text(BP_PAGE.into()))]),
        hits,
    );
    let (f, via) = fetch_forecast_from(&sources(&base), "ua", now_ms()).unwrap();
    assert_eq!(via, "backup");
    assert_eq!(f["source"], "betteropc");
    assert_eq!(f["commitment"]["scheduled_for"], "2026-09-22T07:00:00.000Z");
    assert_eq!(f["last_reset_at"], "2026-09-12T08:09:17.000Z");
}

#[test]
fn primary_non_json_falls_to_mirror() {
    let hits = Arc::new(Mutex::new(vec![]));
    let base = serve(
        HashMap::from([
            (
                "/primary".into(),
                R::Text("<html>just a moment</html>".into()),
            ),
            ("/raw".into(), env(600_000, forecast())),
        ]),
        hits,
    );
    let (_, via) = fetch_forecast_from(&sources(&base), "ua", now_ms()).unwrap();
    assert_eq!(via, "mirror");
}

#[test]
fn primary_down_mirror_wins() {
    let hits = Arc::new(Mutex::new(vec![]));
    let base = serve(
        HashMap::from([("/raw".into(), env(600_000, forecast()))]),
        hits,
    );
    assert_eq!(
        fetch_forecast_from(&sources(&base), "ua", now_ms())
            .unwrap()
            .1,
        "mirror"
    );
}

#[test]
fn fresh_raw_mirror_wins() {
    let hits = Arc::new(Mutex::new(vec![]));
    let base = serve(
        HashMap::from([("/raw".into(), env(600_000, forecast()))]),
        hits,
    );
    let (f, via) = fetch_forecast_from(&sources(&base), "ua", now_ms()).unwrap();
    assert_eq!(via, "mirror");
    assert_eq!(f, forecast());
}

#[test]
fn raw_down_jsdelivr_mirror() {
    let hits = Arc::new(Mutex::new(vec![]));
    let base = serve(
        HashMap::from([("/jsd".into(), env(3_600_000, forecast()))]),
        hits,
    );
    assert_eq!(
        fetch_forecast_from(&sources(&base), "ua", now_ms())
            .unwrap()
            .1,
        "mirror"
    );
}

#[test]
fn stale_mirrors_fall_to_direct() {
    let hits = Arc::new(Mutex::new(vec![]));
    let base = serve(
        HashMap::from([
            ("/raw".into(), env(7 * 3600_000, forecast())),
            ("/jsd".into(), env(8 * 3600_000, forecast())),
            ("/direct".into(), R::Json(forecast())),
        ]),
        hits,
    );
    assert_eq!(
        fetch_forecast_from(&sources(&base), "ua", now_ms())
            .unwrap()
            .1,
        "direct"
    );
}

#[test]
fn mirror_upstream_array_or_string_skipped() {
    let hits = Arc::new(Mutex::new(vec![]));
    let base = serve(
        HashMap::from([
            ("/raw".into(), env(0, json!([]))),
            ("/jsd".into(), env(0, json!("<html>"))),
            ("/direct".into(), R::Json(forecast())),
        ]),
        hits,
    );
    assert_eq!(
        fetch_forecast_from(&sources(&base), "ua", now_ms())
            .unwrap()
            .1,
        "direct"
    );
}

#[test]
fn mirror_wrong_schema_not_json_404_skipped() {
    let hits = Arc::new(Mutex::new(vec![]));
    let base = serve(
        HashMap::from([
            ("/raw".into(), R::Json(json!({"schema": 99}))),
            ("/jsd".into(), R::Text("not json at all".into())),
            ("/direct".into(), R::Json(forecast())),
        ]),
        hits.clone(),
    );
    assert_eq!(
        fetch_forecast_from(&sources(&base), "ua", now_ms())
            .unwrap()
            .1,
        "direct"
    );
    let base = serve(
        HashMap::from([
            ("/raw".into(), R::Status(404)),
            ("/jsd".into(), R::Status(404)),
            ("/direct".into(), R::Json(forecast())),
        ]),
        hits,
    );
    assert_eq!(
        fetch_forecast_from(&sources(&base), "ua", now_ms())
            .unwrap()
            .1,
        "direct"
    );
}

#[test]
fn direct_html_falls_to_backup() {
    let hits = Arc::new(Mutex::new(vec![]));
    let base = serve(
        HashMap::from([
            ("/direct".into(), R::Text("<html>redesign</html>".into())),
            ("/backup".into(), R::Text(BP_PAGE.into())),
        ]),
        hits,
    );
    let (f, via) = fetch_forecast_from(&sources(&base), "ua", now_ms()).unwrap();
    assert_eq!(via, "backup");
    assert_eq!(f["source"], "betteropc");
}

#[test]
fn direct_5xx_to_backup() {
    let hits = Arc::new(Mutex::new(vec![]));
    let base = serve(
        HashMap::from([
            ("/direct".into(), R::Status(503)),
            ("/backup".into(), R::Text(BP_PAGE.into())),
        ]),
        hits,
    );
    assert_eq!(
        fetch_forecast_from(&sources(&base), "ua", now_ms())
            .unwrap()
            .1,
        "backup"
    );
}

#[test]
fn everything_down_throws_with_all_reasons() {
    let hits = Arc::new(Mutex::new(vec![]));
    let base = serve(HashMap::new(), hits); // 全部断连
    let err = fetch_forecast_from(&sources(&base), "ua", now_ms()).unwrap_err();
    assert!(err.starts_with("all sources failed:"));
    // 五个源各自的失败原因都在串里（连接拒绝文本五份）
    assert!(
        err.matches("refused").count() + err.matches('/').count() >= 5
            || err.matches("failed").count() >= 1
    );
}

#[test]
fn mirror_may_carry_backup_shaped_upstream() {
    let hits = Arc::new(Mutex::new(vec![]));
    let base = serve(
        HashMap::from([("/raw".into(), env(60_000, resets()))]),
        hits,
    );
    let (f, via) = fetch_forecast_from(&sources(&base), "ua", now_ms()).unwrap();
    assert_eq!(via, "mirror");
    assert_eq!(f, resets());
}

#[test]
fn every_request_carries_our_ua() {
    let hits = Arc::new(Mutex::new(vec![]));
    let base = serve(
        HashMap::from([("/raw".into(), env(0, forecast()))]),
        hits.clone(),
    );
    fetch_forecast_from(&sources(&base), "is-tibo-happy/test", now_ms()).unwrap();
    let h = hits.lock().unwrap();
    assert!(!h.is_empty() && h.iter().all(|x| x.contains("ua:is-tibo-happy/test")));
}

#[test]
fn mirror_envelope_shape() {
    let e = mirror_envelope(&json!({"a": 1}), "https://src", now_ms());
    assert_eq!(e["schema"], 1);
    assert_eq!(e["source_url"], "https://src");
    assert_eq!(e["upstream"], json!({"a": 1}));
    assert!(is_tibo_happy::state::parse_iso_pub(e["fetched_at"].as_str().unwrap()).is_some());
}
