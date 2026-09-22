// 热更新：查 GitHub Release → 下载 universal2 二进制 → sha256 校验 → --selftest 冒烟
// → 原子替换自己 → 调用方退出非零码，launchd KeepAlive(SuccessfulExit=false) 拉起新版。
// 信任面与 curl|bash 安装等价（同一个 repo 的 HTTPS 产物）；dev 构建与 PATH 外运行不更新。
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

const LATEST_API: &str = "https://api.github.com/repos/elijah7x/is-tibo-happy/releases/latest";
const ASSET_BIN: &str = "is-tibo-happy";
const ASSET_SUMS: &str = "SHA256SUMS";

// 只有"已安装实例"才允许自动更新：exe 必须住在安装目录里，
// repo/target 下的 dev 构建永远不自我替换
pub fn installed(exe_dir: PathBuf) -> bool {
    exe_dir
        .to_string_lossy()
        .contains("Library/Application Support/is-tibo-happy")
}

fn download(url: &str, ua: &str, timeout: Duration) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let r = ureq::get(url)
        .timeout(timeout)
        .set("User-Agent", ua)
        .set("Accept", "application/octet-stream")
        .call()
        .map_err(|e| format!("{e}"))?;
    let mut buf = Vec::new();
    r.into_reader()
        .take(64 * 1024 * 1024) // 二进制上限 64MB，防异常流量
        .read_to_end(&mut buf)
        .map_err(|e| format!("body: {e}"))?;
    Ok(buf)
}

fn selftest(path: &Path) -> Result<(), String> {
    use std::process::{Command, Stdio};
    use wait_timeout::ChildExt;
    let mut child = Command::new(path)
        .arg("--selftest")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("selftest spawn: {e}"))?;
    match child.wait_timeout(Duration::from_secs(15)) {
        Ok(Some(s)) if s.success() => Ok(()),
        Ok(Some(s)) => Err(format!("selftest exit {s}")),
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            Err("selftest timeout".into())
        }
        Err(e) => Err(format!("selftest wait: {e}")),
    }
}

// Ok(Some(ver)) = 已换好新二进制，调用方应退出让 launchd 重启；Ok(None) = 无更新
pub fn check_and_swap(ua: &str) -> Result<Option<String>, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let rel = ureq::get(LATEST_API)
        .timeout(Duration::from_secs(10))
        .set("User-Agent", ua)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("releases/latest: {e}"))?
        .into_json::<Value>()
        .map_err(|e| format!("release json: {e}"))?;
    let tag = rel["tag_name"].as_str().ok_or("release: no tag_name")?;
    let remote = semver::Version::parse(tag.trim_start_matches('v')).map_err(|_| format!("bad tag {tag}"))?;
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION")).map_err(|e| e.to_string())?;
    if remote <= current {
        return Ok(None);
    }
    let asset_url = |name: &str| -> Result<String, String> {
        rel["assets"]
            .as_array()
            .and_then(|a| {
                a.iter()
                    .find(|x| x["name"].as_str() == Some(name))
                    .and_then(|x| x["browser_download_url"].as_str())
            })
            .map(String::from)
            .ok_or_else(|| format!("asset {name} missing"))
    };
    let bin_url = asset_url(ASSET_BIN)?;
    let sums_url = asset_url(ASSET_SUMS)?;
    let bin = download(&bin_url, ua, Duration::from_secs(120))?;
    let sums = String::from_utf8_lossy(&download(&sums_url, ua, Duration::from_secs(15))?).to_string();
    let want = sums
        .lines()
        .find_map(|l| l.split_whitespace().next().filter(|_| l.ends_with(ASSET_BIN)))
        .ok_or("SHA256SUMS lacks binary entry")?;
    let got = format!("{:x}", Sha256::digest(&bin));
    if got != want {
        return Err(format!("sha256 mismatch: {got} != {want}"));
    }
    let tmp = exe.with_file_name("is-tibo-happy.new");
    std::fs::write(&tmp, &bin).map_err(|e| format!("write tmp: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    selftest(&tmp)?; // 跑不起来就别换
    std::fs::rename(&tmp, &exe).map_err(|e| format!("swap: {e}"))?;
    Ok(Some(tag.to_string()))
}
