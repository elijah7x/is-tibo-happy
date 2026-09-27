// SIGUSR1 → Node inspector(127.0.0.1:9229) → 主进程 Runtime.evaluate
//   → webContents.executeJavaScript 的运行时附加通道。
// Electron 的 nodeCliInspect fuse 开着时，运行中的宿主进程吃 SIGUSR1 会开出
// main-process inspector——不需要 --remote-debugging-port 启动参数、不重启、
// 不弹授权框。每次用完必须 inspector.close()：9229 是 main 进程级执行入口，
// 比 renderer CDP 权限更高，绝不留常驻监听。
use crate::cdp::{self, Cdp};
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom};
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
#[cfg(target_os = "macos")]
extern "C" {
    fn proc_pidpath(pid: i32, buffer: *mut u8, buffersize: u32) -> i32;
}

#[cfg(target_os = "macos")]
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

// Linux 等价物：/proc/<pid>/exe 软链由内核维护，直指可执行文件真身
// （mirror workflow 在 Linux 上构建同一 crate 跑 fetch-state，链接必须过）
#[cfg(target_os = "linux")]
fn pid_exe_path(pid: u32) -> Option<PathBuf> {
    std::fs::canonicalize(format!("/proc/{pid}/exe")).ok()
}

// 其他平台：验不出真身就返回 None → inspector 通道走不通，daemon 退回端口模式
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn pid_exe_path(_pid: u32) -> Option<PathBuf> {
    None
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

pub fn framework_bin(app: &Path) -> PathBuf {
    app.join("Contents/Frameworks/Codex Framework.framework/Codex Framework")
}

// Framework 身份键 "mtime:size:ino"：作 supported 缓存键，也作 poison 文件内容——
// 原位升级（同秒 mtime 也可能撞）靠 size/inode 兜底；metadata 取不到 → None = 不缓存
fn framework_key(app: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(framework_bin(app)).ok()?;
    let mt = m
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(format!("{mt}:{}:{}", m.len(), m.ino()))
}

// Electron fuse 线：sentinel + version(1B) + 个数 n(1B) + n 个 ASCII 位。
// index 3 = nodeCliInspect：fuse 关时二进制里 handler 符号仍在但进程不装
// SIGUSR1 handler——符号检查过了不够，必须读线本体（发信号 = 杀进程）。
const FUSE_SENTINEL: &[u8] = b"dL7pKGdnNz796PbbjQWNKmHXBZaB9tsX";
const FUSE_IDX_CLI_INSPECT: usize = 3;

// sentinel 后字节流的纯解析：version 须为 1，n>3，wire[3]∈{'0','1','r'}
fn parse_fuse_wire(tail: &[u8]) -> Option<bool> {
    if tail.len() < 2 || tail[0] != 1 {
        return None;
    }
    let n = tail[1] as usize;
    if n <= FUSE_IDX_CLI_INSPECT || tail.len() < 2 + n {
        return None;
    }
    match tail[2 + FUSE_IDX_CLI_INSPECT] {
        b'1' => Some(true),
        b'0' | b'r' => Some(false),
        _ => None,
    }
}

// 流式扫全文件找 fuse 线（universal 二进制每个 slice 各有一条，必须全查）：
// 1MiB 块 + 64B 重叠防跨块漏检；命中后补读线尾用独立句柄 seek，不动主游标。
// 汇总规则：零命中/任一不可解析 → None；任一 '0'/'r' → Some(false)；全 '1' → Some(true)
pub fn fuse_cli_inspect(path: &Path) -> Option<bool> {
    let mut f = std::fs::File::open(path).ok()?;
    const CHUNK: usize = 1 << 20;
    const OVLP: usize = 64;
    let mut buf = vec![0u8; CHUNK + OVLP];
    let mut carry = 0usize;
    let mut base = 0u64;   // buf[0] 的文件绝对偏移
    let mut consumed = 0u64; // 已读入缓冲的绝对末尾（补读线尾的 seek 起点）
    let mut last_hit: Option<u64> = None; // 重叠区重复命中同一条线按绝对偏移去重
    let mut votes: Vec<bool> = Vec::new();
    loop {
        let n = f.read(&mut buf[carry..carry + CHUNK]).ok()?;
        consumed += n as u64;
        let end = carry + n;
        let mut i = 0;
        while i + FUSE_SENTINEL.len() <= end {
            if buf[i..i + FUSE_SENTINEL.len()] == *FUSE_SENTINEL {
                let abs = base + i as u64;
                if Some(abs) != last_hit {
                    last_hit = Some(abs);
                    let mut tail: Vec<u8> = buf[i + FUSE_SENTINEL.len()..end].to_vec();
                    if tail.len() < 2 || tail.len() < 2 + tail[1] as usize {
                        let mut ext = [0u8; 512];
                        if let Ok(mut g) = std::fs::File::open(path)
                            .and_then(|mut g| g.seek(SeekFrom::Start(consumed)).map(|_| g))
                        {
                            if let Ok(m) = g.read(&mut ext) {
                                tail.extend_from_slice(&ext[..m]);
                            }
                        }
                    }
                    match parse_fuse_wire(&tail) {
                        Some(v) => votes.push(v),
                        None => return None,
                    }
                }
                i += FUSE_SENTINEL.len();
            } else {
                i += 1;
            }
        }
        if n == 0 {
            break;
        }
        carry = OVLP.min(end);
        buf.copy_within(end - carry..end, 0);
        base += (end - carry) as u64;
    }
    if votes.is_empty() {
        None
    } else if votes.iter().all(|&v| v) {
        Some(true)
    } else {
        Some(false)
    }
}

// 熔断文件内容 = Framework 身份键：同版本内 supported() 永假，换版本自动解封
pub fn write_poison(app: &Path, file: &Path) {
    if let Some(k) = framework_key(app) {
        let _ = std::fs::write(file, k);
    }
}
pub fn poisoned(app: &Path, file: &Path) -> bool {
    match framework_key(app) {
        Some(k) => std::fs::read_to_string(file)
            .ok()
            .is_some_and(|s| s.trim() == k),
        None => false,
    }
}

// handler 符号存在性（单独可测）：没有 → 进程没装 handler，盲发 SIGUSR1 会杀死它
pub fn handler_symbol(app: &Path) -> bool {
    // grep -m1 命中即退——strings 不必读完全量
    Command::new("sh")
        .args([
            "-c",
            "strings -a \"$1\" | grep -qm1 StartDebugSignalHandler",
            "sh",
        ])
        .arg(framework_bin(app))
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
        .unwrap_or(false)
}

// 静态 gate（fail closed）：handler 符号 + fuse nodeCliInspect=on + 未被 poison
// 三者齐全才允许发 SIGUSR1。符号+fuse 按 Framework mtime 缓存；poison 每次现查
// （熔断必须即时生效，不能吃到缓存里的旧结论）。
pub fn supported(app: &Path, poison_file: &Path) -> bool {
    static CACHE: Mutex<Option<(String, bool)>> = Mutex::new(None);
    if let Some(key) = framework_key(app) {
        if let Some((k, ok)) = &*CACHE.lock().unwrap() {
            if *k == key {
                return *ok && !poisoned(app, poison_file);
            }
        }
        let bin = framework_bin(app);
        let ok = handler_symbol(app) && fuse_cli_inspect(&bin) == Some(true);
        *CACHE.lock().unwrap() = Some((key, ok));
        return ok && !poisoned(app, poison_file);
    }
    // metadata 取不到：不缓存，符号+fuse 现查，判不出也 fail closed
    let bin = framework_bin(app);
    handler_symbol(app) && fuse_cli_inspect(&bin) == Some(true) && !poisoned(app, poison_file)
}

// SIGUSR1 → 等 inspector 端点 → 连 ws → eval process.pid 验明正身。
// 9229 可能被别的 Node 进程占用：pid 不符立刻断开，绝不在别人的 inspector 上跑码。
pub fn attach(exe: &Path, app: &Path, poison_file: &Path) -> Result<(Cdp, u32), String> {
    // 信号前最后一道闸：宿主可能在两次轮询之间被原位升级（秒级），
    // 挂着的 inspector_ok 是旧构建的结论——kill -USR1 之前必须按当前文件重判
    if !supported(app, poison_file) {
        return Err("inspector unsupported on this build".into());
    }
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

    // fuse 线字节序列：sentinel + version + n + n 个 ASCII 位
    fn fuse_wire(version: u8, bits: &str) -> Vec<u8> {
        let mut v = Vec::from(FUSE_SENTINEL);
        v.push(version);
        v.push(bits.len() as u8);
        v.extend_from_slice(bits.as_bytes());
        v
    }
    fn tail(wire: &[u8]) -> &[u8] {
        &wire[FUSE_SENTINEL.len()..]
    }

    #[test]
    fn fuse_wire_parsing() {
        assert_eq!(parse_fuse_wire(tail(&fuse_wire(1, "010011001"))), Some(false)); // index3='0'
        assert_eq!(parse_fuse_wire(tail(&fuse_wire(1, "011111111"))), Some(true)); // index3='1'
        assert_eq!(parse_fuse_wire(tail(&fuse_wire(1, "010r11001"))), Some(false)); // 'r' = removed
        assert_eq!(parse_fuse_wire(tail(&fuse_wire(2, "011111111"))), None); // version≠1
        assert_eq!(parse_fuse_wire(tail(&fuse_wire(1, "010"))), None); // n=3，index3 不存在
        assert_eq!(parse_fuse_wire(&fuse_wire(1, "010011001")[FUSE_SENTINEL.len()..FUSE_SENTINEL.len() + 5]), None); // 截断
        assert_eq!(parse_fuse_wire(&[]), None);
    }

    #[test]
    fn fuse_cli_inspect_streams_and_finds_sentinel() {
        let dir = std::env::temp_dir().join(format!("ith-fuse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("fw");
        // 无 sentinel → None
        std::fs::write(&f, b"just bytes").unwrap();
        assert_eq!(fuse_cli_inspect(&f), None);
        // sentinel 横跨 1MiB 块边界：pad 到 CHUNK-10，sentinel 前 10B 落第一块、余下进第二块
        let mut blob = vec![b'x'; (1 << 20) - 10];
        blob.extend_from_slice(&fuse_wire(1, "010011001"));
        std::fs::write(&f, &blob).unwrap();
        assert_eq!(fuse_cli_inspect(&f), Some(false));
        blob.truncate((1 << 20) - 10);
        blob.extend_from_slice(&fuse_wire(1, "011111111"));
        std::fs::write(&f, &blob).unwrap();
        assert_eq!(fuse_cli_inspect(&f), Some(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fuse_cli_inspect_multiwire_universal_rules() {
        // universal 二进制每个 slice 一条线：任一 '0' → false；任一不可解析 → None
        let dir = std::env::temp_dir().join(format!("ith-fuse2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("fw");
        let mut b = fuse_wire(1, "011111111"); // 线1: index3='1'
        b.extend_from_slice(&[b'x'; 4096]);
        b.extend_from_slice(&fuse_wire(1, "010011001")); // 线2: index3='0'
        std::fs::write(&f, &b).unwrap();
        assert_eq!(fuse_cli_inspect(&f), Some(false));
        let mut b = fuse_wire(1, "011111111");
        b.extend_from_slice(&fuse_wire(1, "011111111"));
        std::fs::write(&f, &b).unwrap();
        assert_eq!(fuse_cli_inspect(&f), Some(true));
        let mut b = fuse_wire(1, "011111111");
        b.extend_from_slice(&fuse_wire(2, "011111111")); // 线2: version≠1 不可解析
        std::fs::write(&f, &b).unwrap();
        assert_eq!(fuse_cli_inspect(&f), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 假 bundle：{bin内容, poison内容或None} → supported()。每次重写 bin 前睡 1.1s
    // 保证 mtime key 变化，不吃缓存旧值（支持 fuse 写入）
    fn fake_app(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("ith-{tag}-{}", std::process::id()));
        let fw = dir.join("Contents/Frameworks/Codex Framework.framework");
        std::fs::create_dir_all(&fw).unwrap();
        (dir.clone(), fw.join("Codex Framework"), dir.join("inspector-poisoned"))
    }

    #[test]
    fn supported_gates_on_symbol_fuse_and_poison() {
        let (dir, bin, poison) = fake_app("insp");
        // 符号+fuse 都没有 → false
        std::fs::write(&bin, b"no handler here").unwrap();
        assert!(!supported(&dir, &poison));
        thread::sleep(Duration::from_millis(1100));
        // 只有符号、fuse 线缺失 → false（fail closed）
        std::fs::write(&bin, b"blob StartDebugSignalHandler blob").unwrap();
        assert!(!supported(&dir, &poison));
        thread::sleep(Duration::from_millis(1100));
        // 符号 + fuse index3='0'（fuse 关）→ false：符号还在但 handler 不装
        let mut b = Vec::from(&b"sym StartDebugSignalHandler"[..]);
        b.extend_from_slice(&fuse_wire(1, "010011001"));
        std::fs::write(&bin, &b).unwrap();
        assert!(!supported(&dir, &poison));
        thread::sleep(Duration::from_millis(1100));
        // 符号 + fuse index3='1' → true
        let mut b = Vec::from(&b"sym StartDebugSignalHandler"[..]);
        b.extend_from_slice(&fuse_wire(1, "011111111"));
        std::fs::write(&bin, &b).unwrap();
        assert!(supported(&dir, &poison));
        // poison 记当前 key → false；换 key（mtime 变）→ poison 自动失效
        write_poison(&dir, &poison);
        assert!(!supported(&dir, &poison));
        assert!(poisoned(&dir, &poison));
        thread::sleep(Duration::from_millis(1100));
        std::fs::write(&bin, &b).unwrap(); // 同内容，mtime 变 → key 变
        assert!(!poisoned(&dir, &poison));
        assert!(supported(&dir, &poison));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn main_pid_none_for_absent_exe() {
        // 不存在的路径：pgrep 空输出 → None（绝不能盲发信号）
        assert_eq!(main_pid(Path::new("/nonexistent/Codex.app/x")), None);
    }
}

