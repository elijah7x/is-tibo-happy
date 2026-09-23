// is-tibo-happy：单一二进制多子命令。
//   is-tibo-happy [--once|--no-quit|--launch|--no-update]   常驻守护（默认）
//   is-tibo-happy fetch-state [--out PATH]                  GH Actions 镜像任务（写信封 JSON）
//   is-tibo-happy update                                   手动触发一次热更新检查
//   is-tibo-happy probe                                    dev 验证：SIGUSR1 附加活体 App，注入一次后摘出
//   is-tibo-happy --selftest                               下载产物的冒烟自检（更新前验证用）
//   is-tibo-happy --version
use is_tibo_happy::{daemon, net, state, update};
use serde_json::Value;
use std::time::Duration;

// 镜像任务：resets → forecast 两个 JSON 源直连，被拦再走 r.jina.ai 中继
// （拦数据中心 IP），betteropc 页面最后兜底
fn fetch_state(args: &[String]) -> i32 {
    const UA: &str = "is-tibo-happy-mirror/0.2 (+https://github.com/elijah7x/is-tibo-happy)";
    const RELAY: &str = "https://r.jina.ai/";

    let get_json = |url: &str| -> Result<Value, String> {
        ureq::get(url)
            .timeout(Duration::from_secs(20))
            .set("User-Agent", UA)
            .set("Accept", "application/json")
            .call()
            .map_err(|e| format!("{e}"))?
            .into_json::<Value>()
            .map_err(|e| format!("not json: {e}"))
    };
    let get_text = |url: &str| -> Result<String, String> {
        ureq::get(url)
            .timeout(Duration::from_secs(20))
            .set("User-Agent", UA)
            .set("Accept", "text/html,*/*;q=0.8")
            .call()
            .map_err(|e| format!("{e}"))?
            .into_string()
            .map_err(|e| format!("body: {e}"))
    };
    // r.jina.ai 返回 "Title:/URL Source:/Markdown Content:" 封皮 + JSON 正文，按花括号切片
    let get_json_relayed = |url: &str| -> Result<Value, String> {
        let text = ureq::get(&format!("{RELAY}{url}"))
            .timeout(Duration::from_secs(45))
            .set("User-Agent", UA)
            .call()
            .map_err(|e| format!("{e} relay"))?
            .into_string()
            .map_err(|e| format!("relay body: {e}"))?;
        let i = text.find('{').ok_or("relay: no json")?;
        let j = text.rfind('}').ok_or("relay: no json")?;
        if j <= i {
            return Err("relay: no json in body".into());
        }
        serde_json::from_str(&text[i..=j]).map_err(|e| format!("relay json: {e}"))
    };

    let mut upstream: Option<Value> = None;
    let mut src = String::new();
    for (url, relay) in [
        (net::SOURCE_PRIMARY, false),
        (net::SOURCE_DIRECT, false),
        (net::SOURCE_PRIMARY, true),
        (net::SOURCE_DIRECT, true),
    ] {
        let r = if relay {
            get_json_relayed(url)
        } else {
            get_json(url)
        };
        match r {
            Ok(j) => {
                src = url.to_string();
                upstream = Some(j);
                break;
            }
            Err(e) => eprintln!(
                "{}{} failed: {e}",
                if relay { "relay " } else { "" },
                url.split('/').nth(2).unwrap_or(url)
            ),
        }
    }
    if upstream.is_none() {
        match get_text(net::SOURCE_BACKUP) {
            Ok(html) => match state::parse_betteropc(&html) {
                Some(bp) => {
                    src = net::SOURCE_BACKUP.to_string();
                    upstream = Some(bp);
                }
                None => eprintln!("betteropc: page not recognized"),
            },
            Err(e) => eprintln!("betteropc failed: {e}"),
        }
    }
    let Some(upstream) = upstream else { return 1 }; // Actions 标红但不提交，上一份 state.json 保留

    let env = net::mirror_envelope(&upstream, &src, is_tibo_happy::now_ms());
    let out = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "public/state.json".to_string());
    if let Some(dir) = std::path::Path::new(&out).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(&out, serde_json::to_string_pretty(&env).unwrap() + "\n") {
        Ok(()) => {
            println!(
                "state.json updated via {src}: {}",
                upstream["updated_at"].as_str().unwrap_or("no updated_at")
            );
            0
        }
        Err(e) => {
            eprintln!("write {out}: {e}");
            1
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(|s| s.as_str()) {
        Some("--version") | Some("-V") => {
            println!("is-tibo-happy {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some("--selftest") | Some("selftest") => {
            // 新二进制上线前冒烟：能跑、内嵌资源完好即可
            let ok = serde_json::from_str::<Value>(is_tibo_happy::AVATAR_SRC).is_ok()
                && is_tibo_happy::WIDGET_SRC.contains("__ith");
            println!(
                "is-tibo-happy {} selftest {}",
                env!("CARGO_PKG_VERSION"),
                if ok { "ok" } else { "FAIL" }
            );
            if ok {
                0
            } else {
                1
            }
        }
        Some("probe") => daemon::probe(&args),
        Some("fetch-state") => fetch_state(&args),
        Some("update") => {
            // 手动更新同样过安装门禁——repo/target 里的 dev 二进制不能被自我替换
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                .unwrap_or_default();
            if !update::installed(exe_dir) {
                eprintln!("update: only the installed daemon (~/Library/Application Support/is-tibo-happy) self-updates");
                1
            } else {
                match update::check_and_swap("is-tibo-happy/0.2 (manual update)") {
                    Ok(Some(v)) => {
                        println!("updated to {v} — restart the daemon to run it");
                        0
                    }
                    Ok(None) => {
                        println!("already up to date ({})", env!("CARGO_PKG_VERSION"));
                        0
                    }
                    Err(e) => {
                        eprintln!("update failed: {e}");
                        1
                    }
                }
            }
        }
        _ => daemon::run(&args),
    };
    std::process::exit(code);
}
