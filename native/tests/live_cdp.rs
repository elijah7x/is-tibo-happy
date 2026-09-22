// 真机 CDP 验证：需要本机 ChatGPT.app 正带 --remote-debugging-port=9333 运行。
// `cargo test -- --ignored` 手动触发；CI 不跑。
use is_tibo_happy::cdp::*;
use std::time::Duration;

#[test]
#[ignore]
fn cdp_live_target_and_eval() {
    let targets = list_targets(PORT).expect("/json unreachable");
    let t = targets
        .iter()
        .find(|t| {
            t.get("type").and_then(|x| x.as_str()) == Some("page")
                && t.get("url").and_then(|u| u.as_str()).map_or(false, |u| {
                    u.starts_with("app://")
                        && u.contains("-/index.html")
                        && !u.contains("initialRoute")
                })
        })
        .expect("app target not found");
    let ws = t["webSocketDebuggerUrl"].as_str().unwrap();
    let c = Cdp::connect(ws, Duration::from_secs(8)).unwrap();

    let href = eval_js(&c, "location.href").unwrap();
    assert!(
        href.as_str().unwrap_or("").starts_with("app://"),
        "unexpected href: {href}"
    );

    // widget 注入路径：写状态 + eval widget 源码 → __ith 必须为真
    eval_js(&c, "window.__ith_state = window.__ith_state || { mood: 'HAPPY', mainLine: 'cdp test', subLine: '', offline: false };").unwrap();
    let src = include_str!("../../src/widget.js");
    eval_js(&c, src).unwrap();
    let ok = eval_js(&c, "window.__ith ? 1 : 0").unwrap();
    assert_eq!(ok.as_i64(), Some(1));
    // 重复注入幂等：先销毁旧实例再重建，不抛错
    eval_js(&c, src).unwrap();
    let ok = eval_js(&c, "window.__ith ? 1 : 0").unwrap();
    assert_eq!(ok.as_i64(), Some(1));
    c.close();
}
