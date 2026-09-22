// SIGUSR1 → Node inspector(127.0.0.1:9229) → 主进程 Runtime.evaluate
//   → webContents.executeJavaScript 的运行时附加通道。
// Electron 的 nodeCliInspect fuse 开着时，运行中的宿主进程吃 SIGUSR1 会开出
// main-process inspector——不需要 --remote-debugging-port 启动参数、不重启、
// 不弹授权框。每次用完必须 inspector.close()：9229 是 main 进程级执行入口，
// 比 renderer CDP 权限更高，绝不留常驻监听。
use crate::cdp::{self, Cdp};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, UNIX_EPOCH};
use wait_timeout::ChildExt;

pub const PORT: u16 = 9229;

// pgrep -f 只给候选集：它匹配 argv 任意位置，无关进程 argv 里提到路径也会命中；
// 且 argv 可被进程自改（setproctitle 伪装）。发 SIGUSR1 前必须用内核报告的
// proc_pidpath 验明可执行文件真身——SIGUSR1 对非 Node 进程默认动作是终止。
extern "C" {
    fn proc_pidpath(pid: i32, buffer: *mut u8, buffersize: u32) -> i32;
}

fn pid_exe_path(pid: u32) -> Option<PathBuf> {
    let mut buf = [0u8; 4096];
    let n = unsafe { proc_pidpath(pid as i32, buf.as_mut_ptr(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    let raw = &buf[..(n as usize).min(buf.len())];
    let raw = raw.split(|&b| b == 0).next().unwrap_or(raw);
    let p = PathBuf::from(String::from_utf8_lossy(raw).into_owned());
    std::fs::canonicalize(p).ok()
}

pub fn main_pid(exe: &Path) -> Option<u32> {
    let want = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    crate::daemon::run_cmd(
        "pgrep",
        &["-f", &exe.to_string_lossy()],
        Duration::from_secs(5),
    )
    .ok()?
    .lines()
    .filter_map(|l| l.trim().parse::<u32>().ok())
    .find(|&pid| pid_exe_path(pid) == Some(want.clone()))
}

// 静态 gate：Framework 里没有 node 的 StartDebugSignalHandler → 进程没装
// handler，SIGUSR1 走默认动作会把它杀死——盲发不得。按 Framework mtime 缓存判定。
pub fn supported(app: &Path) -> bool {
    static CACHE: Mutex<Option<(u64, bool)>> = Mutex::new(None);
    let bin = app.join("Contents/Frameworks/Codex Framework.framework/Codex Framework");
    let key = std::fs::metadata(&bin)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if key != 0 {
        if let Some((k, ok)) = *CACHE.lock().unwrap() {
            if k == key {
                return ok;
            }
        }
    }
    // grep -m1 命中即退——strings 不必读完全量
    let ok = Command::new("sh")
        .args([
            "-c",
            "strings -a \"$1\" | grep -qm1 StartDebugSignalHandler",
            "sh",
        ])
        .arg(&bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
        .map(|mut c| match c.wait_timeout(Duration::from_secs(30)) {
            Ok(Some(s)) => s.success(),
            _ => {
                let _ = c.kill();
                let _ = c.wait();
                false
            }
        })
        .unwrap_or(false);
    if key != 0 {
        *CACHE.lock().unwrap() = Some((key, ok));
    }
    ok
}

// SIGUSR1 → 等 inspector 端点 → 连 ws → eval process.pid 验明正身。
// 9229 可能被别的 Node 进程占用：pid 不符立刻断开，绝不在别人的 inspector 上跑码。
pub fn attach(exe: &Path) -> Result<(Cdp, u32), String> {
    let pid = main_pid(exe).ok_or("app main process not found")?;
    let _ = Command::new("kill")
        .args(["-USR1", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let mut ws = None;
    for _ in 0..50 {
        if let Ok(list) = cdp::list_targets(PORT) {
            if let Some(u) = list
                .iter()
                .find_map(|t| t["webSocketDebuggerUrl"].as_str())
                .map(String::from)
            {
                ws = Some(u);
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    let conn = Cdp::connect(
        &ws.ok_or("inspector endpoint never appeared")?,
        Duration::from_secs(10),
    )?;
    match eval_main(&conn, "process.pid") {
        Ok(v) if v.as_u64() == Some(pid as u64) => Ok((conn, pid)),
        r => {
            conn.close();
            Err(format!("inspector owner mismatch: {r:?}"))
        }
    }
}

// 关 inspector：close() 会强断所有活动连接并等服务器停止——在当前 eval 里直调
// 会自锁（我们的调用本身占着一条连接），先 setTimeout 排后再断开 ws。
// 然后有界确认端口真的关了：调用方马上再 attach 会撞上还没执行的 close
// 定时器被连坐强杀；9229 是 main 进程级入口，关失败必须可见。
pub fn detach(conn: &Cdp) -> bool {
    let _ = eval_main(
        conn,
        "setTimeout(()=>process.mainModule.require('node:inspector').close(),50)",
    );
    conn.close();
    for _ in 0..20 {
        thread::sleep(Duration::from_millis(80));
        if cdp::list_targets(PORT).is_err() {
            return true; // 端口已拒绝连接 = inspector 服务器已关
        }
    }
    false
}

// 主进程 eval：awaitPromise + returnByValue（executeJavaScript 返回 Promise）
pub fn eval_main(conn: &Cdp, expression: &str) -> Result<Value, String> {
    let r = conn.send(
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "awaitPromise": true,
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
        return Err(format!("main eval: {} {}", e["text"], desc));
    }
    Ok(r.get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(Value::Null))
}

// 主窗口 webContents：app://-/index.html 且不含 initialRoute
// （avatar-overlay / detached-window 同为 index.html 但带路由参数）
const MAIN_WC: &str = "process.mainModule.require('electron').webContents.getAllWebContents()\
    .find(w=>{const u=w.getURL();return u.startsWith('app://-/index.html')&&!u.includes('initialRoute=')})";

// 在宿主主窗口页面里跑 JS（等价旧通道里直接 Runtime.evaluate 页面目标）
pub fn eval_page(conn: &Cdp, page_expr: &str) -> Result<Value, String> {
    let src = serde_json::to_string(page_expr).map_err(|e| e.to_string())?;
    eval_main(
        conn,
        &format!(
            "(()=>{{const w={MAIN_WC};return w?w.executeJavaScript({src}):Promise.reject(new Error('no main window'))}})()"
        ),
    )
}

// widget 侧的 ithRefresh 替身：inspector 用完即关，没有常驻 binding——
// refresh/menu-unmatched 事件排进 __ithPendingRefresh，下次 attach 由 drain_pending 收走
const REFRESH_SHIM: &str = "window.ithRefresh=window.ithRefresh||function(k){try{\
    var p=window.__ithPendingRefresh=window.__ithPendingRefresh||[];\
    p.push({k:String(k||''),t:Date.now()});if(p.length>50)p.splice(0,p.length-50)\
    }catch(e){}}";

// 页面重载后要回放的 restore 表达式（头像+最新态）。main 进程侧存 globalThis，
// did-finish-load 钩子注入 widget 后照它回放——新 widget 不裸奔到下次推送
pub fn restore_expr(avatar: Option<&str>, state: Option<&Value>) -> String {
    let mut s = String::new();
    if let Some(av) = avatar {
        s.push_str(&format!("window.__ith&&window.__ith.setAvatar({av});"));
    }
    if let Some(st) = state {
        s.push_str(&format!("window.__ith&&window.__ith.setState({st});"));
    }
    s
}

// 初始化：注册 App 级重注入钩子 + 更新 restore 存量 + shim + 当前页立即注入。
// 钩子挂 main 进程 globalThis.__ithInit 幂等——daemon 重启不重复注册；
// web-contents-created 覆盖窗口重建，did-finish-load 覆盖页面重载。
// 注意：web-contents-created 触发时 c.getURL() 恒为空（Electron #15040），
// URL 判定必须放进 did-finish-load 回调里做——无条件挂钩、load 时再过滤。
pub fn init_page(
    conn: &Cdp,
    widget_src: &str,
    avatar: Option<&str>,
    cached_state: Option<&Value>,
) -> Result<(), String> {
    // reload 后页面上下文整个重建：shim 必须并进重注入载荷，不能只在初次注入时给
    let payload = serde_json::to_string(&format!("{REFRESH_SHIM}\n{widget_src}"))
        .map_err(|e| e.to_string())?;
    let restore = serde_json::to_string(&restore_expr(avatar, cached_state))
        .map_err(|e| e.to_string())?;
    eval_main(conn, &format!(
        "(()=>{{const e=process.mainModule.require('electron');\
        if((globalThis.__ithInit||0)<2){{\
        globalThis.__ithInit=2;\
        const ok=c=>{{const u=c.getURL();return u.startsWith('app://-/index.html')&&!u.includes('initialRoute=')}};\
        const inj=c=>c.executeJavaScript({payload}).then(()=>c.executeJavaScript(globalThis.__ithRestore||'')).catch(()=>{{}});\
        const hook=c=>c.on('did-finish-load',()=>{{if(!c.isDestroyed()&&ok(c))inj(c)}});\
        e.app.on('web-contents-created',(ev,c)=>hook(c));\
        e.webContents.getAllWebContents().forEach(hook);\
        }}\
        globalThis.__ithRestore={restore};\
        return 'ok'}})()"
    ))?;
    // 当前页立即注入；页面尚在加载时 executeJavaScript 可能失败——did-finish-load 兜住
    let _ = inject_page(conn, widget_src);
    let _ = eval_page(conn, &restore_expr(avatar, cached_state));
    Ok(())
}

// 页面侧注入（shim+widget 同源载荷）。窗口重建后主窗口还在、widget 没了，
// push 路径探活发现时用它就地补注，不等下轮 init。
pub fn inject_page(conn: &Cdp, widget_src: &str) -> Result<Value, String> {
    eval_page(conn, &format!("{REFRESH_SHIM}\n{widget_src}"))
}

// 收走页面侧排队的 ithRefresh 事件，顺带探活：widget 的 setState 是函数才算活着。
// 窗口重建（进程不退、wc 换新）后 __ith 消失——探活并进这个每次必跑的 eval，
// 否则指纹去重命中时永远不 eval，widget 丢了无人知晓。
// 返回 (widget_alive, [(kind, 时间戳ms)])
pub fn drain_pending(conn: &Cdp) -> (bool, Vec<(String, i64)>) {
    let v = eval_page(conn, "(()=>{const p=window.__ithPendingRefresh||[];window.__ithPendingRefresh=[];return JSON.stringify({a:!!(window.__ith&&window.__ith.setState),p})})()")
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .and_then(|s| serde_json::from_str::<Value>(&s).ok());
    let alive = v.as_ref().and_then(|v| v["a"].as_bool()) == Some(true);
    let events = v
        .and_then(|v| v["p"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|e| {
            (
                e["k"].as_str().unwrap_or("").to_string(),
                e["t"].as_i64().unwrap_or(0),
            )
        })
        .collect();
    (alive, events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_expr_composes_avatar_and_state() {
        let st = json!({"kind":"happy","detail":{"sub":"x"}});
        let e = restore_expr(Some("{\"variants\":{}}"), Some(&st));
        assert!(e.contains("setAvatar({\"variants\":{}})"));
        assert!(e.contains("setState(") && e.contains("\"kind\":\"happy\""));
        // 空输入 → 空串（钩子回放空表达式无副作用）
        assert_eq!(restore_expr(None, None), "");
        // 只有 state 没有头像也要成串
        let only = restore_expr(None, Some(&st));
        assert!(!only.contains("setAvatar"));
        assert!(only.contains("setState"));
    }

    #[test]
    fn supported_gates_on_framework_symbol() {
        // 假 bundle：framework 二进制里没有 handler 符号 → false；有 → true
        let dir = std::env::temp_dir().join(format!("ith-insp-{}", std::process::id()));
        let fw = dir.join("Contents/Frameworks/Codex Framework.framework");
        std::fs::create_dir_all(&fw).unwrap();
        let bin = fw.join("Codex Framework");
        std::fs::write(&bin, b"no handler here").unwrap();
        assert!(!supported(&dir));
        // 缓存放 key=mtime：等一秒改写保证新 key，避免吃到上面的缓存
        thread::sleep(Duration::from_millis(1100));
        std::fs::write(&bin, b"blob StartDebugSignalHandler blob").unwrap();
        assert!(supported(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn main_pid_none_for_absent_exe() {
        // 不存在的路径：pgrep 空输出 → None（绝不能盲发信号）
        assert_eq!(main_pid(Path::new("/nonexistent/Codex.app/x")), None);
    }
}

