// 热更新链路的隔离测试：本地 TcpListener 假 release 服务器，验证
// check_and_swap_at 的下载→sha256→selftest→原子替换 全路径与失败路径。
// 生产端点是 ITH_UPDATE_API 缺省时的 GitHub releases/latest；
// 替换目标经 check_and_swap_at 注入临时文件，不能落在测试二进制自己身上。
use is_tibo_happy::update::*;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

// env var 是进程全局的——所有改 env 的用例必须串行
static LOCK: Mutex<()> = Mutex::new(());
static SEQ: AtomicU64 = AtomicU64::new(0);

struct Fake {
    base: String,
}

// 起本地服务器：/releases/latest → 构造好的 release JSON；其余路径 → files 表
fn serve(tag: &str, files: Vec<(&'static str, Vec<u8>)>) -> Fake {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let rel = json!({
        "tag_name": tag,
        "assets": [
            {"name": "is-tibo-happy", "browser_download_url": format!("{base}/is-tibo-happy")},
            {"name": "SHA256SUMS", "browser_download_url": format!("{base}/SHA256SUMS")},
        ]
    })
    .to_string();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(mut s) = conn else { break };
            let mut buf = [0u8; 8192];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let path = req.split_whitespace().nth(1).unwrap_or("/");
            let body = if path == "/releases/latest" {
                Some(rel.clone().into_bytes())
            } else {
                files
                    .iter()
                    .find(|(p, _)| *p == path.trim_start_matches('/'))
                    .map(|(_, b)| b.clone())
            };
            match body {
                Some(b) => {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        b.len()
                    );
                    let _ = s.write_all(head.as_bytes());
                    let _ = s.write_all(&b);
                }
                None => {
                    let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                }
            }
        }
    });
    Fake { base }
}

fn ino(p: &PathBuf) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).unwrap().ino()
}

fn real_bin() -> Vec<u8> {
    std::fs::read(env!("CARGO_BIN_EXE_is-tibo-happy")).unwrap()
}

fn sums_for(bin: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    format!("{:x}  is-tibo-happy\n", Sha256::digest(bin)).into_bytes()
}

// 独立临时 exe 当替换目标：每用例唯一路径，不碰测试进程自己的二进制
fn tmp_exe() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "ith-update-test-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&p, real_bin()).unwrap();
    p
}

// 清掉代理 env：ureq 会吃 ALL_PROXY/http_proxy 之类，CI runner 上这些变量
// 时有时无，挂着时 loopback 请求被劫去代理导致 EINVAL/超时（门禁曾因此挂过）。
const PROXY_VARS: [&str; 8] = [
    "http_proxy", "https_proxy", "all_proxy", "no_proxy",
    "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY",
];

struct ApiGuard(Vec<(&'static str, Option<String>)>);
impl Drop for ApiGuard {
    fn drop(&mut self) {
        std::env::remove_var("ITH_UPDATE_API");
        for (k, v) in self.0.drain(..) {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }
}
fn use_api(base: &str) -> ApiGuard {
    let saved = PROXY_VARS
        .iter()
        .map(|k| (*k, std::env::var(k).ok()))
        .collect::<Vec<_>>();
    for k in PROXY_VARS {
        std::env::remove_var(k);
    }
    std::env::set_var("ITH_UPDATE_API", format!("{base}/releases/latest"));
    ApiGuard(saved)
}

#[test]
fn swap_installs_newer_release_and_binary_runs() {
    let _g = LOCK.lock().unwrap();
    let bin = real_bin();
    let f = serve(
        "v99.0.0",
        vec![
            ("is-tibo-happy", bin.clone()),
            ("SHA256SUMS", sums_for(&bin)),
        ],
    );
    let _api = use_api(&f.base);
    let exe = tmp_exe();
    let before = ino(&exe);
    assert_eq!(check_and_swap_at(&exe, "ith-test"), Ok(Some("v99.0.0".into())));
    // 同字节也要能证明替换发生：rename 必然换 inode
    assert_ne!(before, ino(&exe));
    // 落下来的产物必须能跑自检（N+1 用的是真实 is-tibo-happy 二进制）
    assert!(std::process::Command::new(&exe)
        .arg("--selftest")
        .status()
        .unwrap()
        .success());
    let _ = std::fs::remove_file(&exe);
}

#[test]
fn swap_rejects_bad_sha256_and_keeps_old_binary() {
    let _g = LOCK.lock().unwrap();
    let bin = b"fake binary".to_vec();
    let f = serve(
        "v99.0.0",
        vec![
            ("is-tibo-happy", bin),
            ("SHA256SUMS", b"deadbeef  is-tibo-happy\n".to_vec()),
        ],
    );
    let _api = use_api(&f.base);
    let exe = tmp_exe();
    let before = ino(&exe);
    assert!(check_and_swap_at(&exe, "ith-test")
        .unwrap_err()
        .contains("sha256"));
    assert_eq!(before, ino(&exe));
    let _ = std::fs::remove_file(&exe);
}

#[test]
fn swap_rejects_failing_selftest() {
    let _g = LOCK.lock().unwrap();
    let bin = b"#!/bin/sh\nexit 1\n".to_vec();
    let f = serve(
        "v99.0.0",
        vec![
            ("is-tibo-happy", bin.clone()),
            ("SHA256SUMS", sums_for(&bin)),
        ],
    );
    let _api = use_api(&f.base);
    let exe = tmp_exe();
    let before = ino(&exe);
    assert!(check_and_swap_at(&exe, "ith-test")
        .unwrap_err()
        .contains("selftest"));
    assert_eq!(before, ino(&exe));
    let _ = std::fs::remove_file(&exe);
    let _ = std::fs::remove_file(exe.with_file_name("is-tibo-happy.new"));
}

#[test]
fn no_swap_when_remote_not_newer() {
    let _g = LOCK.lock().unwrap();
    let bin = real_bin();
    // 远端同版 → Ok(None) 且原文件不动
    let f = serve(
        concat!("v", env!("CARGO_PKG_VERSION")),
        vec![
            ("is-tibo-happy", bin.clone()),
            ("SHA256SUMS", sums_for(&bin)),
        ],
    );
    let _api = use_api(&f.base);
    let exe = tmp_exe();
    let before = ino(&exe);
    assert_eq!(check_and_swap_at(&exe, "ith-test"), Ok(None));
    assert_eq!(before, ino(&exe));
    let _ = std::fs::remove_file(&exe);
}

#[test]
fn missing_asset_is_error_not_crash() {
    let _g = LOCK.lock().unwrap();
    let f = serve("v99.0.0", vec![]); // release JSON 指向的文件不存在 → 404
    let _api = use_api(&f.base);
    let exe = tmp_exe();
    assert!(check_and_swap_at(&exe, "ith-test").is_err());
    let _ = std::fs::remove_file(&exe);
}

#[test]
fn garbage_release_json_is_error_not_panic() {
    let _g = LOCK.lock().unwrap();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(mut s) = conn else { break };
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nxxxxx");
        }
    });
    let _api = use_api(&base);
    let exe = tmp_exe();
    assert!(check_and_swap_at(&exe, "ith-test").is_err());
    let _ = std::fs::remove_file(&exe);
}
