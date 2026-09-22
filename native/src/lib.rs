// is-tibo-happy 库本体：state/net/cdp/update/daemon 全在这，main.rs 只是 CLI 壳。
pub mod cdp;
pub mod daemon;
pub mod inspector;
pub mod net;
pub mod state;
pub mod update;

use serde_json::Value;

// widget.js 与头像打进二进制：安装载荷 = 一个二进制 + 一个 plist
pub const WIDGET_SRC: &str = include_str!("../../src/widget.js");
pub const AVATAR_SRC: &str = include_str!("../../assets/avatar/avatar.json");

pub fn avatar_json() -> Option<String> {
    serde_json::from_str::<Value>(AVATAR_SRC)
        .ok()
        .map(|v| v.to_string())
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
