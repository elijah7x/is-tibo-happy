// 发版门禁专用假 release 服务器（CI/本地仿真用，不进任何运行时路径）：
//   GET /releases/latest → {"tag_name": <tag>, assets: [is-tibo-happy, SHA256SUMS]}
//   GET /<file>          → <dir>/<file> 的字节
// 用法: ith-fake-release <dir> <tag> <port>
use std::io::{Read, Write};
use std::net::TcpListener;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, tag, port) = (&args[1], &args[2], &args[3]);
    let l = TcpListener::bind(format!("127.0.0.1:{port}")).expect("bind");
    let base = format!("http://127.0.0.1:{port}");
    let rel = format!(
        r#"{{"tag_name":"{tag}","assets":[{{"name":"is-tibo-happy","browser_download_url":"{base}/is-tibo-happy"}},{{"name":"SHA256SUMS","browser_download_url":"{base}/SHA256SUMS"}}]}}"#
    );
    println!("fake-release listening on {base} tag={tag}");
    for conn in l.incoming() {
        let Ok(mut s) = conn else { continue };
        let mut buf = [0u8; 8192];
        let n = s.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        let path = req.split_whitespace().nth(1).unwrap_or("/");
        let body = if path == "/releases/latest" {
            Some(rel.clone().into_bytes())
        } else {
            std::fs::read(format!("{dir}/{}", path.trim_start_matches('/'))).ok()
        };
        match body {
            Some(b) => {
                let _ = s.write_all(
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len()).as_bytes(),
                );
                let _ = s.write_all(&b);
            }
            None => {
                let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
            }
        }
    }
}
