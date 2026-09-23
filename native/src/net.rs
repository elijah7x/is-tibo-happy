// 拉取链：codex-resets.com/api/resets 是第一信源——scheduled 预告 + events 落地
// 记录（含 reset_type:"banked" 发卡，forecast 端点刻意不收）一条响应全覆盖；
// 其下是 GitHub Actions 镜像（raw → 境内 jsDelivr CDN，每 20min 归一化），
// 再退 codex-reset.com forecast JSON，最后 betteropc 页面解析兜底。
// 每一环都有界超时。返回 (forecast, via)；forecast 一律是对象（betteropc 页面在
// fetch 时就地归一化，不往缓存塞 HTML），交给 derive() 按形状分派。
use crate::state::parse_betteropc;
use serde_json::{json, Value};
use std::time::Duration;

pub const REPO: &str = "elijah7x/is-tibo-happy";
pub const SOURCE_PRIMARY: &str = "https://codex-resets.com/api/resets";
pub const SOURCE_DIRECT: &str = "https://codex-reset.com/api/forecast";
pub const SOURCE_BACKUP: &str = "https://betteropc.com/ai-products/reset-signals/codex";

// 信源可注入：生产用 default()，测试用本地 stub 服务器替换
pub struct Sources {
    pub primary: String,
    pub mirrors: Vec<String>,
    pub direct: String,
    pub backup: String,
}
impl Default for Sources {
    fn default() -> Self {
        Self {
            primary: SOURCE_PRIMARY.into(),
            mirrors: vec![
                format!("https://raw.githubusercontent.com/{REPO}/main/public/state.json"),
                format!("https://cdn.jsdelivr.net/gh/{REPO}@main/public/state.json"),
            ],
            direct: SOURCE_DIRECT.into(),
            backup: SOURCE_BACKUP.into(),
        }
    }
}

const MIRROR_MAX_AGE_MS: i64 = 6 * 3600_000; // 镜像超过 6h 视为过期，降级直连

fn host(url: &str) -> &str {
    url.split('/').nth(2).unwrap_or(url)
}

fn status_of(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, _) => code.to_string(),
        ureq::Error::Transport(t) => t.to_string(),
    }
}

fn get_json(url: &str, ua: &str, timeout: Duration) -> Result<Value, String> {
    let r = ureq::get(url)
        .timeout(timeout)
        .set("User-Agent", ua)
        .set("Accept", "application/json")
        .call()
        .map_err(|e| format!("{} {}", status_of(&e), host(url)))?;
    r.into_json::<Value>().map_err(|e| format!("not json: {e}"))
}

fn get_text(url: &str, ua: &str, timeout: Duration) -> Result<String, String> {
    let r = ureq::get(url)
        .timeout(timeout)
        .set("User-Agent", ua)
        .set("Accept", "text/html,*/*;q=0.8")
        .call()
        .map_err(|e| format!("{} {}", status_of(&e), host(url)))?;
    r.into_string().map_err(|e| format!("body: {e}"))
}

// 镜像信封是否新鲜可用（schema 1 + upstream 对象 + fetched_at 在 6h 内）
pub fn mirror_fresh(env: &Value, now: i64) -> bool {
    env.get("schema").and_then(|s| s.as_i64()) == Some(1)
        && env.get("upstream").is_some_and(|u| u.is_object())
        && env
            .get("fetched_at")
            .and_then(|f| f.as_str())
            .and_then(crate::state::parse_iso_pub)
            .is_some_and(|f| now - f < MIRROR_MAX_AGE_MS)
}

pub fn fetch_forecast_from(
    srcs: &Sources,
    ua: &str,
    now: i64,
) -> Result<(Value, &'static str), String> {
    let mut errors: Vec<String> = Vec::new();
    match get_json(&srcs.primary, ua, Duration::from_secs(12)) {
        Ok(j) => return Ok((j, "primary")),
        Err(e) => errors.push(e),
    }
    for url in &srcs.mirrors {
        match get_json(url, ua, Duration::from_secs(8)) {
            Ok(env) if mirror_fresh(&env, now) => return Ok((env["upstream"].clone(), "mirror")),
            Ok(_) => errors.push("mirror stale/invalid".into()),
            Err(e) => errors.push(e),
        }
    }
    match get_json(&srcs.direct, ua, Duration::from_secs(15)) {
        Ok(j) => return Ok((j, "direct")),
        Err(e) => errors.push(e),
    }
    match get_text(&srcs.backup, ua, Duration::from_secs(12)) {
        Ok(html) => match parse_betteropc(&html) {
            Some(bp) => return Ok((bp, "backup")),
            None => errors.push("betteropc: unrecognized page".into()),
        },
        Err(e) => errors.push(e),
    }
    Err(format!("all sources failed: {}", errors.join("; ")))
}

pub fn fetch_forecast(ua: &str, now: i64) -> Result<(Value, &'static str), String> {
    fetch_forecast_from(&Sources::default(), ua, now)
}

// 镜像侧（GH Actions）写出的信封格式——工具与客户端共用同一 schema。
pub fn mirror_envelope(upstream: &Value, source_url: &str, now: i64) -> Value {
    json!({
        "schema": 1,
        "fetched_at": crate::state::iso_pub(now),
        "source_url": source_url,
        "upstream": upstream,
    })
}
