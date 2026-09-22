// is-tibo-happy 主控：启动宿主 → CDP 注入 widget → 抓数据推状态 → 断线自愈。
//   is-tibo-happy            常驻守护（LaunchAgent 语境）
//   --once    注入+推一次状态后退出
//   --no-quit App 已运行但没开调试端口时，不自动重启它，直接报错退出
//   --launch  手动授权：App 没在跑也拉起
//   --no-update 关闭每日热更新检查
// 资源纪律：App 缺席时只在首装/--launch 下拉起（退出不复活）。附加优先级：
// 调试端口已在 → 常驻 CDP 会话；在跑但没端口 → SIGUSR1 走 Node inspector 附加
// （零重启零弹窗）；inspector 不可用才回退"静默重启一次挂端口"。
use crate::cdp::{self, Cdp, CdpEvent};
use crate::inspector;
use crate::net::fetch_forecast;
use crate::state::{derive, resolve_display, sub_line};
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use wait_timeout::ChildExt;

const PORT: u16 = cdp::PORT;
const UA: &str = "is-tibo-happy/0.2 (+https://github.com/elijah7x/is-tibo-happy)"; // 自报家门：让源站能认出、限流、联系我们
const POLL: Duration = Duration::from_secs(15 * 60); // 上游数据轮询
const RECONNECT: Duration = Duration::from_secs(3); // CDP 断开后重试间隔
const APP_POLL: Duration = Duration::from_secs(30); // App 不在时的等待轮询
const RELAUNCH_CAP: usize = 3; // 每小时最多重启 App 次数（防打架循环）
const RELAUNCH_WINDOW: Duration = Duration::from_secs(3600);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
macro_rules! log {
    ($($a:tt)*) => { println!("[is-tibo-happy {}] {}", chrono::Local::now().format("%H:%M:%S"), format!($($a)*)) };
}

// 推送日志截断：按字符数取（对齐 JS slice 语义）——按字节切会在多字节 UTF-8 边界 panic
fn brief120(d: &Value) -> String {
    serde_json::to_string(d)
        .unwrap_or_default()
        .chars()
        .take(120)
        .collect()
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

// 锁文件固定在安装目录：仓库 checkout 与安装版共用一把锁，防双实例同时推（审计 🟡-2）
fn pid_file() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/Application Support/is-tibo-happy/is-tibo-happy.pid")
}

// execSync 等价物：跑子进程并拿 stdout，超时就杀（子进程挂起不能冻住主循环）
pub(crate) fn run_cmd(prog: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{prog}: {e}"))?;
    match child.wait_timeout(timeout) {
        Ok(Some(_)) => {
            let mut out = String::new();
            let _ = child.stdout.take().map(|mut s| s.read_to_string(&mut out));
            Ok(out)
        }
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(format!("{prog}: timeout"))
        }
        Err(e) => Err(format!("{prog}: {e}")),
    }
}

fn find_app() -> Option<PathBuf> {
    // 宿主可能装在 /Applications 或用户级 ~/Applications
    [
        "/Applications/ChatGPT.app".to_string(),
        format!(
            "{}/Applications/ChatGPT.app",
            std::env::var("HOME").unwrap_or_default()
        ),
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.exists())
}
fn app_exe(app: &Path) -> PathBuf {
    app.join("Contents/MacOS/ChatGPT")
}

fn acquire_lock() {
    let pidfile = pid_file();
    if let Some(dir) = pidfile.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = std::fs::read_to_string(&pidfile) {
        if let Ok(pid) = s.trim().parse::<u32>() {
            // pid 可能被无关进程复用 → 只比可执行名（comm= 不含 args，
            // 防止 `cat …/is-tibo-happy/x` 这类无关进程被误认领成守护实例）
            let ours = run_cmd(
                "ps",
                &["-p", &pid.to_string(), "-o", "comm="],
                Duration::from_secs(5),
            )
            .map(|o| {
                Path::new(o.trim()).file_name().and_then(|f| f.to_str()) == Some("is-tibo-happy")
            })
            .unwrap_or(false);
            if ours {
                eprintln!("[is-tibo-happy] already running (pid {pid})");
                std::process::exit(1);
            }
        }
    }
    let _ = std::fs::write(&pidfile, std::process::id().to_string());
    // pidfile 退出时不清（进程可随时被硬杀）：上面的 comm= 身份校验兜底残留文件的正确性
}

fn port_up() -> bool {
    cdp::get_version(PORT).is_ok()
}

fn app_running(exe: &Path) -> bool {
    run_cmd(
        "pgrep",
        &["-f", &exe.to_string_lossy()],
        Duration::from_secs(5),
    )
    .map(|o| !o.trim().is_empty())
    .unwrap_or(false)
}

// with_port=false：inspector 通道可用时的拉起——不带调试端口，成功判据是主进程出现
fn spawn_app(exe: &Path, with_port: bool) -> bool {
    if with_port {
        log!("launching with debug port {PORT}");
    } else {
        log!("launching app (inspector attach on next poll)");
    }
    let mut c = Command::new(exe);
    if with_port {
        c.arg(format!("--remote-debugging-port={PORT}"));
    }
    let _ = c
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    for _ in 0..45 {
        thread::sleep(Duration::from_secs(1));
        let up = if with_port {
            port_up()
        } else {
            inspector::main_pid(exe).is_some()
        };
        if up {
            return true;
        }
    }
    false
}

// 近一小时内主动重启 App 的次数；两类拉起（重启带端口 / first-run 冷启）共用同一 cap
struct RelaunchCap {
    times: Vec<Instant>,
    logged: bool, // cap 封顶日志只打一次，窗口滑出后重置
}
impl RelaunchCap {
    fn allowed(&mut self) -> bool {
        let now = Instant::now();
        self.times
            .retain(|t| now.duration_since(*t) < RELAUNCH_WINDOW);
        if self.times.len() < RELAUNCH_CAP {
            self.logged = false;
        }
        if self.times.len() >= RELAUNCH_CAP {
            if !self.logged {
                log!("relaunch cap reached; waiting for app to come back on its own");
                self.logged = true;
            }
            return false;
        }
        self.times.push(now);
        true
    }
}

struct Daemon {
    exe: PathBuf,
    no_quit: bool,
    no_update: bool,
    once: bool,
    stopping: Arc<AtomicBool>,
    cap: Mutex<RelaunchCap>,
    // 会话内状态
    active: Arc<Mutex<Option<Arc<Cdp>>>>,
    injected: AtomicBool,
    pushing: AtomicBool,
    // inspector 通道（SIGUSR1 → Node inspector）：二进制有 handler 且 ITH_NO_INSPECT 未设才启用；
    // attach 连续失败会降级回端口模式。inspector 模式下无常驻连接，attach→干活→close 口。
    inspector_ok: AtomicBool,
    hooked_pid: AtomicU32, // 已完成 init_page 的宿主主进程 pid
    insp_lock: Mutex<()>,  // 串行化 attach/detach——inspector.close 会误杀并存的另一会话
    app_seen_at: Mutex<Option<Instant>>, // 首次观察到 App 在跑的时刻——识别"启动中"的 attach 竞态
    init_fails: AtomicU32,               // init_page 连续失败数——驱动退避（不算 attach 三振）
    next_init_at: Mutex<Option<Instant>>, // init 退避：此刻前不再试 init，防永久失败无限 flap
    last_pushed: Mutex<String>,
    last_fetch_at: Mutex<i64>,
    last_fetch_ok: AtomicBool,
    last_err: Mutex<String>,
    cache: Mutex<Value>, // {state, at} 或 Null
    cache_file: PathBuf,
    first_flag: PathBuf,
    tz: Option<String>,
}

// inspector_init 的三态：宽限（Deferred）既不算成功也不记三振——owner-mismatch
// 和启动竞态不该洗掉已累积的 attach 失败计数（宽限推迟 ≠ 既往不咎）
enum InitRes {
    Ok,
    Deferred,
    Failed,
}

impl Daemon {
    fn fetch_state(&self) -> Value {
        let now = now_ms();
        let mut fresh = None;
        match fetch_forecast(UA, now) {
            Ok((forecast, via)) => {
                *self.last_fetch_at.lock().unwrap() = now_ms();
                let mut s = derive(&forecast, now_ms());
                s["detail"]["sub"] = sub_line(&s, now_ms(), self.tz.as_deref());
                s["detail"]["via"] = json!(via);
                self.last_fetch_ok.store(true, Ordering::Relaxed);
                fresh = Some(s);
            }
            Err(e) => {
                self.last_fetch_ok.store(false, Ordering::Relaxed);
                let mut le = self.last_err.lock().unwrap();
                if *le != e {
                    log!("fetch failed: {e}");
                    *le = e;
                }
            }
        }
        let (state, cache) = {
            let c = self.cache.lock().unwrap();
            resolve_display(
                fresh.as_ref(),
                if c.is_null() { None } else { Some(&c) },
                now_ms(),
                self.tz.as_deref(),
            )
        };
        if cache != *self.cache.lock().unwrap() {
            *self.cache.lock().unwrap() = cache.clone();
            // 原子写：tmp+rename，进程被杀不丢半个 JSON
            let tmp = self.cache_file.with_extension("tmp");
            if std::fs::write(&tmp, cache.to_string()).is_ok() {
                let _ = std::fs::rename(&tmp, &self.cache_file);
            }
        }
        state
    }

    // 不走网络的展示态：60s 内取过的数据直接重解析。init 重试热路径用——
    // attach 失败时白跑一轮 5 源 fetch（最坏 ~58s 超时预算）纯属浪费
    fn display_state(&self) -> Value {
        if now_ms() - *self.last_fetch_at.lock().unwrap() < 60_000 {
            let c = self.cache.lock().unwrap();
            return resolve_display(
                None,
                if c.is_null() { None } else { Some(&c) },
                now_ms(),
                self.tz.as_deref(),
            )
            .0;
        }
        self.fetch_state()
    }

    fn eval(&self, expr: &str) -> Result<Value, String> {
        let conn = self.active.lock().unwrap().clone();
        match conn {
            Some(c) => cdp::eval_js(&c, expr),
            None => Err("no active connection".into()),
        }
    }

    fn push(&self) {
        if !self.pushing.swap(true, Ordering::SeqCst) {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.push_inner()));
            self.pushing.store(false, Ordering::SeqCst);
            if r.is_err() {
                log!("push panicked");
            }
        }
    }

    // 指纹去重→经 eval 闭包推上屏；返回 Err 供调用方识别"主窗口没了"这类可恢复失败。
    // 状态由调用方预先取好——fetch 最坏 ~58s 网络等待，不该开着 inspector 端口做
    fn push_via(&self, s: &Value, eval: impl Fn(&str) -> Result<Value, String>) -> Result<(), String> {
        // 内容指纹：只有用户可见信息变化才重推
        let d = &s["detail"];
        let key = json!({
            "k": s["kind"], "sub": d["sub"],
            "d": d["daysSince"].as_f64().map(|x| x.floor()),
            "s": d["scheduledISO"], "w": d["windowEnd"], "t": d["targetStart"],
        })
        .to_string();
        if *self.last_pushed.lock().unwrap() == key {
            return Ok(());
        }
        let via = d.get("via").and_then(|v| v.as_str()).unwrap_or("-");
        let brief = brief120(d);
        let payload = format!("window.__ith && window.__ith.setState({})", s); // 探活由 drain 的 a 位负责
        match eval(&payload) {
            // 推送成功后才记指纹——失败/中断不吞状态，下一轮重试同一状态
            Ok(_) => {
                *self.last_pushed.lock().unwrap() = key;
                log!(
                    "state: {} via={via} {}",
                    s["kind"].as_str().unwrap_or("?"),
                    brief
                );
                Ok(())
            }
            Err(e) => {
                log!("setState failed: {e}");
                Err(e)
            }
        }
    }

    fn push_inner(&self) {
        if self.inspector_ok.load(Ordering::Relaxed) && self.prefer_inspector() {
            // inspector 通道：无常驻连接——attach→会话→detach。
            // attach 失败不能静默吞（钩子已挂时状态会永久停滞）：记日志+清 hooked，
            // 引回主循环里被三振计数/启动宽限约束的 init 路径
            // 未挂上且 init 在退避期：push 侧也别白 attach——widget 死着推了也没人接，
            // 恢复信号由退避到期后的 init 路径确认（load 钩子会让 widget 先自己长回来）
            if self.hooked_pid.load(Ordering::Relaxed) == 0 && self.init_backoff() {
                return;
            }
            // 取数在会话外做（同 inspector_init 的理由）
            let s = self.fetch_state();
            let _g = self.insp_lock.lock().unwrap();
            match inspector::attach(&self.exe) {
                Ok((conn, _)) => {
                    // 探活成功 = 恢复证据：清掉残留的 init 退避（钩子在窗口期已自己注入）
                    if self.inspector_session(&conn, true, &s) {
                        self.note_init_ok();
                    }
                    if !inspector::detach(&conn) {
                        log!("inspector close unconfirmed — 9229 may still be open");
                    }
                }
                Err(e) => {
                    log!("inspector attach (push): {e}");
                    self.hooked_pid.store(0, Ordering::Relaxed);
                }
            }
            return;
        }
        if self.active.lock().unwrap().is_none() {
            return; // 断开期间不抓远端
        }
        let s = self.fetch_state();
        let _ = self.push_via(&s, |e| self.eval(e));
    }

    // 端口没在才走 inspector（ITH_FORCE_INSPECT 可强制优先，dev 验证用）
    fn prefer_inspector(&self) -> bool {
        std::env::var_os("ITH_FORCE_INSPECT").is_some() || !port_up()
    }

    // inspector 会话内的完整一轮：drain（收事件+探活）→ 必要时就地补注 →
    // 推态 → 同步 restore 存量。s 是会话外预先取好的展示态。
    // reinit_on_missing=true 用于推送路径：补注也挂=页面不可写，清 hooked 交回
    // 主循环 init（计退避）；init 刚注过则 false——页面在加载属正常，load 钩子兜住。
    // 返回值 = 会话末 widget 是否确认活着（探活或补注成功且推态无误）。
    // 调用方据此决定 init 退避计数的走向——不看它就会把"页面已死"误记成"init 成功"
    fn inspector_session(&self, conn: &Cdp, reinit_on_missing: bool, s: &Value) -> bool {
        let (alive, events) = inspector::drain_pending(conn);
        for (k, _) in events {
            if let Some(label) = k.strip_prefix("menu-unmatched:") {
                log!("widget: menu-unmatched:{label}");
            } else {
                log!("refresh via menu (queued)");
            }
        }
        if !alive {
            if inspector::inject_page(conn, crate::WIDGET_SRC).is_ok() {
                log!("widget missing (window rebuilt) — re-injected");
            } else {
                log!("widget missing, re-inject failed — {}", if reinit_on_missing {
                    "re-init later"
                } else {
                    "load hook will cover"
                });
                if reinit_on_missing {
                    self.hooked_pid.store(0, Ordering::Relaxed);
                    self.note_init_fail();
                }
                return false; // 页面无 widget 时 setState 静默 no-op，推了也是吞状态
            }
        }
        let r = self.push_via(s, |e| inspector::eval_page(conn, e));
        // 同步 restore 存量：页面重载后钩子回放最新展示态，不裸奔到下轮推送。
        // 不绑推送成败——restore 存的是"最新已知态"，页面死了也照常写
        let av = crate::avatar_json();
        let restore = inspector::restore_expr(av.as_deref(), Some(s));
        let _ = inspector::eval_main(
            conn,
            &format!(
                "globalThis.__ithRestore={}",
                serde_json::to_string(&restore).unwrap_or_default()
            ),
        );
        // 主窗口被重建（wc 换新）→ 清 hooked 标记，主循环下轮重挂
        if r.as_ref().err().is_some_and(|e| e.contains("no main window")) {
            self.hooked_pid.store(0, Ordering::Relaxed);
        }
        r.is_ok()
    }

    // init_page 失败退避：不算 attach 三振（连接本身好着，是页面侧没就绪/改版），
    // 但永久失败也不能每轮 SIGUSR1 开 inspector——30s 起翻倍封顶 5min。
    fn init_backoff(&self) -> bool {
        self.next_init_at
            .lock()
            .unwrap()
            .is_some_and(|t| Instant::now() < t)
    }

    fn note_init_ok(&self) {
        self.init_fails.store(0, Ordering::Relaxed);
        *self.next_init_at.lock().unwrap() = None;
    }

    fn note_init_fail(&self) {
        let n = self.init_fails.fetch_add(1, Ordering::Relaxed) + 1;
        let secs = [30u64, 60, 120, 300][(n.saturating_sub(1)).min(3) as usize];
        log!("inspector init backing off {secs}s (x{n})");
        *self.next_init_at.lock().unwrap() = Some(Instant::now() + Duration::from_secs(secs));
    }

    // inspector 通道初始化：SIGUSR1 附加 → 注册重注入钩子+注入 → 同连接收事件+
    // 首轮推送 → detach。三态返回：Ok = 挂上且 widget 活着；Failed = attach 失败
    // （累计后降级端口模式）；Deferred = 宽限不记三振也不清零——竞态/蹲坑不算
    // 既往不咎，三振计数跨宽限存活。init 失败多半页面没就绪，计退避不算 attach 失败。
    fn inspector_init(&self) -> InitRes {
        let Some(pid) = inspector::main_pid(&self.exe) else {
            return InitRes::Deferred; // 主进程不在是上层竞态，不算失败也不算成功
        };
        if self.hooked_pid.load(Ordering::Relaxed) == pid {
            return InitRes::Ok;
        }
        // 网络取数在 inspector 会话外做：fetch 最坏 ~58s 超时预算，
        // 不该开着 9229（main 进程级入口）等网络；重试时复用近期取数
        let s = self.display_state();
        {
            let _g = self.insp_lock.lock().unwrap();
            let (conn, pid) = match inspector::attach(&self.exe) {
                Ok(v) => v,
                Err(e) => {
                    // attach 失败一律歇 30s：不歇就每 5s 重试一轮
                    *self.next_init_at.lock().unwrap() =
                        Some(Instant::now() + Duration::from_secs(30));
                    // 9229 被无关 inspector 占用：不是宿主能力问题——不记三振，
                    // 降级路径会为一个蹲坑的进程重启用户 App，太亏
                    if e.contains("owner mismatch") {
                        log!("inspector attach deferred (foreign inspector): {e}");
                        return InitRes::Deferred;
                    }
                    // 同 pid 且刚起 = 启动中竞态，不记 strike；
                    // pid 没了/换了 = 可能是信号把它杀了（无 handler 的最坏情形）
                    // ——记 strike 触发降级止损，别拿 SIGUSR1 反复戳它
                    let young = self
                        .app_seen_at
                        .lock()
                        .unwrap()
                        .map_or(false, |t| t.elapsed() < Duration::from_secs(120));
                    if young && inspector::main_pid(&self.exe) == Some(pid) {
                        log!("inspector attach deferred (app starting): {e}");
                        return InitRes::Deferred;
                    }
                    log!("inspector attach: {e}");
                    return InitRes::Failed;
                }
            };
            let av = crate::avatar_json();
            let r = inspector::init_page(
                &conn,
                crate::WIDGET_SRC,
                av.as_deref(),
                Some(&s),
            );
            match r {
                Ok(()) => {
                    self.hooked_pid.store(pid, Ordering::Relaxed);
                    // 首装授权到此兑现——不消费的话用户每次关 Codex 都会被重新拉起
                    let _ = std::fs::remove_file(&self.first_flag);
                    *self.last_pushed.lock().unwrap() = String::new();
                    log!("inspector attached pid {pid} — widget live, zero restart");
                    // detach→再 attach 会撞上宿主侧 50ms inspector.close() 定时器被
                    // 强杀（真机首推命中率仅 40%）——首轮推送在同一连接内完成；
                    // catch_unwind 同 push()：panic 不得把 pushing 卡在 true
                    // 探活结果决定退避计数走向：挂上但 widget 死了不算 init 成功
                    // （否则死窗口场景退避会被 init_ok 反复清零、停在 30s 档）
                    let mut live = None;
                    if !self.pushing.swap(true, Ordering::SeqCst) {
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            self.inspector_session(&conn, false, &s)
                        }));
                        self.pushing.store(false, Ordering::SeqCst);
                        match r {
                            Ok(l) => live = Some(l),
                            Err(_) => log!("inspector session panicked"),
                        }
                    }
                    match live {
                        Some(true) => self.note_init_ok(),
                        Some(false) => self.note_init_fail(),
                        None => {} // 另一路正在推：探活无结论，退避维持原样
                    }
                }
                Err(e) => {
                    self.note_init_fail();
                    log!("inspector init deferred: {e}");
                }
            }
            if !inspector::detach(&conn) {
                log!("inspector close unconfirmed — 9229 may still be open");
            }
        }
        InitRes::Ok
    }

    fn inject(&self, cdp_conn: &Arc<Cdp>) -> Result<(), String> {
        let _ = self.active.lock().unwrap().replace(cdp_conn.clone());
        cdp::eval_js(cdp_conn, crate::WIDGET_SRC)?;
        self.injected.store(true, Ordering::Relaxed);
        if let Some(av) = crate::avatar_json() {
            let _ = cdp::eval_js(cdp_conn, &format!("window.__ith.setAvatar({av})"));
        }
        // 缓存态先上屏，不等首轮 fetch（网络慢时菜单不至于空白）
        let cache = self.cache.lock().unwrap().clone();
        if let Some(state) = cache.get("state") {
            let _ = cdp::eval_js(
                cdp_conn,
                &format!("window.__ith && window.__ith.setState({state})"),
            );
        }
        *self.last_pushed.lock().unwrap() = String::new(); // 换页/重连后强制重推一次
        self.push();
        log!("widget injected +avatar");
        Ok(())
    }

    fn wait_main_target(&self) -> Result<Value, String> {
        for _ in 0..60 {
            if let Ok(list) = cdp::list_targets(PORT) {
                if let Some(t) = list.into_iter().find(|x| {
                    x["type"].as_str() == Some("page")
                        && x["url"]
                            .as_str()
                            .is_some_and(|u| u.starts_with("app://-/index.html"))
                        && !x["url"]
                            .as_str()
                            .is_some_and(|u| u.contains("initialRoute="))
                }) {
                    return Ok(t);
                }
            }
            thread::sleep(Duration::from_secs(1));
        }
        Err("main window target never appeared".into())
    }

    // 返回 true = 调试端口可用。App 在跑但没端口 → 静默退出再以带端口参数拉起
    // （pkill 信号，不走 Apple Events，无授权弹窗）；launch_if_absent=false 时
    // App 没在跑就原地等待，不主动拉起。
    fn ensure_app(&self, launch_if_absent: bool) -> Result<bool, String> {
        if port_up() {
            return Ok(true);
        }
        if app_running(&self.exe) {
            if self.no_quit {
                return Err("app is running without debug port; quit it or drop --no-quit".into());
            }
            if !self.cap.lock().unwrap().allowed() {
                return Ok(false);
            }
            log!("app running without debug port; restarting it once");
            // 信号而非 osascript：Apple Events 会弹"想要控制 Codex"授权框，
            // 同 uid 进程的信号不需要任何授权——用户不该看到这条提示
            let _ = run_cmd(
                "pkill",
                &["-TERM", "-f", &self.exe.to_string_lossy()],
                Duration::from_secs(5),
            );
            for _ in 0..15 {
                if !app_running(&self.exe) {
                    break;
                }
                thread::sleep(Duration::from_secs(1));
            }
            if app_running(&self.exe) {
                // TERM 没被理（挂起/慢退出）→ 不升级 KILL：装饰性挂件不值得为个端口
                // 杀用户进程。留着它，下轮再试；真退出了说明是我们的 TERM 生效。
                log!("app ignored SIGTERM — leaving it alone, retry next poll");
                return Ok(false);
            }
            return Ok(spawn_app(&self.exe, true));
        }
        if !launch_if_absent || !self.cap.lock().unwrap().allowed() {
            return Ok(false);
        }
        Ok(spawn_app(&self.exe, true))
    }

    // 一轮会话：连上主窗口 → 注入 → 挂到断开为止
    fn session(&self) -> Result<(), String> {
        let target = self.wait_main_target()?;
        let ws = target["webSocketDebuggerUrl"].as_str().ok_or("no ws url")?;
        let conn = Arc::new(Cdp::connect(ws, Duration::from_secs(15))?);
        *self.active.lock().unwrap() = Some(conn.clone());
        log!("main window: {}", target["id"].as_str().unwrap_or("?"));
        conn.send("Runtime.enable", json!({}), Duration::from_secs(10))?;
        conn.send("Page.enable", json!({}), Duration::from_secs(10))?;
        // widget 每次成功挂卡 → window.ithRefresh('') → 这里收到通知后节流刷新
        conn.send(
            "Runtime.addBinding",
            json!({"name": "ithRefresh"}),
            Duration::from_secs(10),
        )?;
        self.inject(&conn)?;

        if self.once {
            thread::sleep(Duration::from_millis(500));
            let _ = cdp::eval_js(&conn, "window.__ith && window.__ith.destroy()");
            return Ok(());
        }
        let mut last_in = Instant::now(); // 最近入站帧时刻——TCP 半开假活探测用
        loop {
            match conn.recv_event(Duration::from_secs(1)) {
                Ok(CdpEvent::Message { method, params }) => {
                    last_in = Instant::now();
                    match method.as_str() {
                        "Runtime.bindingCalled" => {
                            if params["name"].as_str() != Some("ithRefresh") {
                                continue;
                            }
                            let payload = params["payload"].as_str().unwrap_or("");
                            if let Some(label) = payload.strip_prefix("menu-unmatched:") {
                                log!("widget: menu-unmatched:{label}");
                                continue;
                            }
                            if now_ms() - *self.last_fetch_at.lock().unwrap() < 5 * 60 * 1000 {
                                continue; // 5min 节流
                            }
                            log!("refresh via menu");
                            self.push();
                        }
                        "Page.frameNavigated" => {
                            if params["frame"]["parentId"].is_null() {
                                self.injected.store(false, Ordering::Relaxed);
                                log!("navigated, re-injecting");
                                if let Err(e) = self.inject(&conn) {
                                    log!("re-inject failed: {e}");
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Ok(CdpEvent::Closed) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break, // ws 读线程死了 → 断开会话重连
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if self.stopping.load(Ordering::Relaxed) {
                        // 优雅退出：摘掉 widget 再断
                        let _ = cdp::eval_js(&conn, "window.__ith && window.__ith.destroy()");
                        conn.close();
                        std::process::exit(0);
                    }
                    // TCP 半开假活：90s 无入站帧 → 主动 ping 探活，死了就断开会话交回主循环重连
                    if last_in.elapsed() > Duration::from_secs(90) {
                        match conn.send("Browser.getVersion", json!({}), Duration::from_secs(10)) {
                            Ok(_) => last_in = Instant::now(),
                            Err(_) => return Err("cdp keepalive timeout".into()),
                        }
                    }
                }
            }
        }
        self.active.lock().unwrap().take();
        self.injected.store(false, Ordering::Relaxed);
        log!("cdp disconnected");
        conn.close();
        Ok(())
    }

    fn poll_loop(&self) {
        // 拉取失败退避：1min 起 ×2，上限 15min（JS retryTimer 等价物）
        let mut retry = Duration::from_secs(60);
        let mut last_update_check = Instant::now() - Duration::from_secs(24 * 3600); // 首轮轮询即检查更新
        while !self.stopping.load(Ordering::Relaxed) {
            self.push();
            if self.active.lock().unwrap().is_some() && !self.injected.load(Ordering::Relaxed) {
                if let Some(c) = self.active.lock().unwrap().clone() {
                    let _ = self.inject(&c);
                }
            }
            // 热更新：每日一次，只检查已安装实例（exe 在安装目录内）
            if !self.no_update
                && last_update_check.elapsed() >= Duration::from_secs(24 * 3600)
                && crate::update::installed(self.exe_dir_marker())
            {
                last_update_check = Instant::now();
                match crate::update::check_and_swap(UA) {
                    Ok(Some(v)) => {
                        log!("updated to {v} — restarting via launchd");
                        std::process::exit(1); // KeepAlive(SuccessfulExit=false) 会拉起新版
                    }
                    Ok(None) => {}
                    Err(e) => log!("self-update failed: {e}"),
                }
            }
            let wait = if self.last_fetch_ok.load(Ordering::Relaxed) {
                retry = Duration::from_secs(60);
                POLL
            } else {
                let w = retry;
                retry = (retry * 2).min(Duration::from_secs(15 * 60));
                w
            };
            sleep_seg(wait, &self.stopping);
        }
    }

    fn exe_dir_marker(&self) -> PathBuf {
        exe_dir()
    }
}

// 分段 sleep：SIGTERM 到达 1s 内就能醒——整段睡死会让 launchd bootout/升级
// 等到超时升 SIGKILL，widget 失去优雅摘除机会（会话外实测退出延迟 25s+）
fn sleep_seg(dur: Duration, stopping: &AtomicBool) {
    let mut left = dur;
    while left > Duration::ZERO && !stopping.load(Ordering::Relaxed) {
        let step = left.min(Duration::from_secs(1));
        thread::sleep(step);
        left -= step;
    }
}

pub fn run(args: &[String]) -> i32 {
    let once = args.iter().any(|a| a == "--once");
    let no_quit = args.iter().any(|a| a == "--no-quit");
    let force_launch = args.iter().any(|a| a == "--launch");
    let no_update =
        args.iter().any(|a| a == "--no-update") || std::env::var_os("ITH_NO_UPDATE").is_some();

    let dir = exe_dir();
    let logfile = dir.join("daemon.log");
    // 日志防膨胀：>1MB 截断重开
    if std::fs::metadata(&logfile)
        .map(|m| m.len() > 1024 * 1024)
        .unwrap_or(false)
    {
        let _ = std::fs::File::create(&logfile);
    }
    let Some(app) = find_app() else {
        // 宿主没装：睡了再退，避免 launchd KeepAlive 把它拉成 tight loop
        log!("ChatGPT.app not found in /Applications or ~/Applications; exiting");
        thread::sleep(Duration::from_secs(60));
        return 0;
    };
    acquire_lock();

    let stopping = Arc::new(AtomicBool::new(false));
    {
        let stopping = stopping.clone();
        let _ = ctrlc::set_handler(move || {
            stopping.store(true, Ordering::Relaxed);
        });
    }

    let cache_file = dir.join("state-cache.json");
    let cache = std::fs::read_to_string(&cache_file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);

    let daemon = Arc::new(Daemon {
        exe: app_exe(&app),
        no_quit,
        no_update,
        once,
        stopping: stopping.clone(),
        cap: Mutex::new(RelaunchCap {
            times: Vec::new(),
            logged: false,
        }),
        active: Arc::new(Mutex::new(None)),
        injected: AtomicBool::new(false),
        pushing: AtomicBool::new(false),
        inspector_ok: AtomicBool::new(false),
        hooked_pid: AtomicU32::new(0),
        insp_lock: Mutex::new(()),
        app_seen_at: Mutex::new(None),
        init_fails: AtomicU32::new(0),
        next_init_at: Mutex::new(None),
        last_pushed: Mutex::new(String::new()),
        last_fetch_at: Mutex::new(0),
        last_fetch_ok: AtomicBool::new(false),
        last_err: Mutex::new(String::new()),
        cache: Mutex::new(cache),
        cache_file,
        first_flag: dir.join(".first-run"),
        tz: iana_time_zone::get_timezone().ok(),
    });

    // 轮询 + 热更新 线程。panic = 整个进程退出（exit 1 → launchd KeepAlive 拉起），
    // 对齐 JS 版"未捕获异常即崩溃重启"的语义——线程悄悄死掉会让守护进程假活。
    {
        let d = daemon.clone();
        thread::spawn(move || {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| d.poll_loop())).is_err() {
                eprintln!("[is-tibo-happy] poll thread panicked — exiting for launchd restart");
                std::process::exit(1);
            }
        });
    }

    let mut session_fails = 0u32;
    let mut attach_fails = 0u32;
    let d = daemon.clone();
    // inspector 通道：Framework 里有 SIGUSR1 handler 才启用（ITH_NO_INSPECT 可强制回退老路）
    if std::env::var_os("ITH_NO_INSPECT").is_none() && inspector::supported(&app) {
        d.inspector_ok.store(true, Ordering::Relaxed);
        log!("inspector channel available — zero-restart attach");
    }
    while !stopping.load(Ordering::Relaxed) {
        if !app_running(&d.exe) {
            d.hooked_pid.store(0, Ordering::Relaxed);
            *d.app_seen_at.lock().unwrap() = None;
            attach_fails = 0; // 缺席期间旧的 attach 失败一并作废
            d.note_init_ok(); // 同理：新进程可能换了版本，init 退避从头算
            // 进程换代可能换了宿主版本——重新评估 inspector 能力（supported 按
            // Framework mtime 缓存，没变体不重扫），降级过也能自愈回来
            if std::env::var_os("ITH_NO_INSPECT").is_none() {
                d.inspector_ok.store(inspector::supported(&app), Ordering::Relaxed);
            }
            // App 缺席：仅"安装后首跑"（标记文件）或 --launch 授权才拉起；
            // 之后用户退出就只等不拉（缺席拉起 = 退出后它自己又弹回来，太打扰）
            let may_launch = force_launch || d.first_flag.exists();
            if may_launch && d.cap.lock().unwrap().allowed() && spawn_app(&d.exe, !d.inspector_ok.load(Ordering::Relaxed)) {
                // 首装授权已兑现为一次拉起——消费掉，此后缺席永不再拉
                let _ = std::fs::remove_file(&d.first_flag);
            }
            if once {
                break;
            }
            sleep_seg(APP_POLL, &stopping);
            continue;
        }
        {
            let mut seen = d.app_seen_at.lock().unwrap();
            if seen.is_none() {
                *seen = Some(Instant::now());
            }
        }
        // 端口没在且 inspector 可用 → SIGUSR1 附加通道，绝不重启进程
        if d.prefer_inspector() && d.inspector_ok.load(Ordering::Relaxed) {
            // 退避期跳过 init 但不清 attach_fails——否则退避会把三振计数静默洗掉
            if d.init_backoff() {
                // fallthrough：不 init，直接走 wait/once 收尾
            } else {
                match d.inspector_init() {
                    InitRes::Ok => attach_fails = 0,
                    // 宽限不记也不洗：owner-mismatch/启动竞态推迟止损而非清零
                    InitRes::Deferred => {}
                    InitRes::Failed => {
                        attach_fails += 1;
                        if attach_fails >= 3 {
                            d.inspector_ok.store(false, Ordering::Relaxed);
                            log!("inspector attach x{attach_fails} failed — falling back to debug-port mode");
                        }
                    }
                }
            }
            // inspector 模式的 --once 不摘 widget：钩子与 restore 已登记在 main 进程，
            // 摘掉反而留空窗——留着的正是产品要的效果
            if once {
                break;
            }
            // 还没挂上（App 启动中/init 推迟）轮快一点，挂上后回正常节奏
            let wait = if d.hooked_pid.load(Ordering::Relaxed) == 0 {
                Duration::from_secs(5)
            } else {
                APP_POLL
            };
            sleep_seg(wait, &stopping);
            continue;
        }
        // legacy 通道：端口已在（直接开会话）或 inspector 不可用（静默重启兜底）
        let may_launch = force_launch || d.first_flag.exists();
        let mut up = false;
        match d.ensure_app(may_launch) {
            Ok(v) => up = v,
            Err(e) => log!("ensureApp: {e}"),
        }
        if !up {
            if once {
                break;
            }
            sleep_seg(APP_POLL, &stopping);
            continue;
        }
        let _ = std::fs::remove_file(&d.first_flag); // 一次性授权已消费（不等 session，防残留循环拉起）
        match d.session() {
            Ok(()) => session_fails = 0, // 正常断线：不算契约失效
            Err(e) => {
                session_fails += 1;
                // 连续建不起会话（端口被占/宿主改版/注入失败）→ 指数退避，3s 起封顶 15min
                let delay = (RECONNECT * 2u32.saturating_pow(session_fails - 1))
                    .min(Duration::from_secs(15 * 60));
                log!(
                    "session failed (x{session_fails}): {e} — retry in {}s",
                    delay.as_secs()
                );
                if once || stopping.load(Ordering::Relaxed) {
                    break;
                }
                sleep_seg(delay, &stopping);
                continue;
            }
        }
        if once || stopping.load(Ordering::Relaxed) {
            break;
        }
        sleep_seg(RECONNECT, &stopping);
    }
    0
}

// dev 验证探针：SIGUSR1 附加 → 枚举 webContents → 注入 → 摘出。不占 pid 锁不常驻。
// --reload：注入后触发主窗口 reload，等 5s 再查——验证 did-finish-load 重注入+restore 回放
pub fn probe(args: &[String]) -> i32 {
    let Some(app) = find_app() else {
        eprintln!("probe: app not found");
        return 1;
    };
    let exe = app_exe(&app);
    println!("app: {}", app.display());
    println!("handler symbol: {}", inspector::supported(&app));
    let Some(pid) = inspector::main_pid(&exe) else {
        eprintln!("probe: app not running");
        return 1;
    };
    println!("main pid: {pid}");
    match inspector::attach(&exe) {
        Ok((conn, _)) => {
            if args.iter().any(|a| a == "--reinit") {
                // 清幂等标记让新钩子注册生效（旧 listener 仍在但注入幂等，无妨）
                let _ = inspector::eval_main(&conn, "delete globalThis.__ithInit");
            }
            match inspector::eval_main(&conn, "JSON.stringify(process.mainModule.require('electron').webContents.getAllWebContents().map(w=>({id:w.id,url:w.getURL().slice(0,80)})))") {
                Ok(v) => println!("webContents: {}", v.as_str().unwrap_or("?")),
                Err(e) => println!("enumerate failed: {e}"),
            }
            let av = crate::avatar_json();
            let cache: Value = std::fs::read_to_string(exe_dir().join("state-cache.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(Value::Null);
            // --reload 会污染 __ithRestore——先备份（init_page 自身也会写它）
            let _ = inspector::eval_main(&conn, "globalThis.__ithRestoreBak=globalThis.__ithRestore||null");
            match inspector::init_page(&conn, crate::WIDGET_SRC, av.as_deref(), cache.get("state")) {
                Ok(()) => println!("init_page ok"),
                Err(e) => println!("init_page: {e}"),
            }
            match inspector::eval_page(&conn, "document.title") {
                Ok(v) => println!("page eval: {}", v),
                Err(e) => println!("page eval failed: {e}"),
            }
            match inspector::eval_page(&conn, "JSON.stringify({ith:typeof window.__ith,refresh:typeof window.ithRefresh,card:!!document.querySelector('.ith-card'),pending:(window.__ithPendingRefresh||[]).length})") {
                Ok(v) => println!("widget: {}", v.as_str().unwrap_or("?")),
                Err(e) => println!("widget check failed: {e}"),
            }
            let (alive, pending) = inspector::drain_pending(&conn);
            println!("widget alive: {alive}");
            for (k, t) in &pending {
                println!("pending event: k={k:?} t={t}");
            }
            if args.iter().any(|a| a == "--reload") {
                // 塞可辨认 marker 进 restore：reload 后应被钩子回放出来
                let _ = inspector::eval_main(&conn, "globalThis.__ithRestore=\"window.__ith&&window.__ith.setState({kind:'happy',detail:{sub:'probe-restore-ok'}});window.__ithProbeRestored=1;\"");
                let _ = inspector::eval_main(
                    &conn,
                    "process.mainModule.require('electron').webContents.getAllWebContents()\
                     .find(w=>{const u=w.getURL();return u.startsWith('app://-/index.html')&&!u.includes('initialRoute=')})\
                     .reload()",
                );
                println!("reloading main page, waiting 6s…");
                thread::sleep(Duration::from_secs(6));
                match inspector::eval_page(&conn, "JSON.stringify({ith:typeof window.__ith,refresh:typeof window.ithRefresh,restored:window.__ithProbeRestored||0})") {
                    Ok(v) => println!("after reload: {}", v.as_str().unwrap_or("?")),
                    Err(e) => println!("after reload check failed: {e}"),
                }
                // 还原 restore 存量，并把页面上的探针假文案也用真实态盖掉
                let _ = inspector::eval_main(&conn, "globalThis.__ithRestore=globalThis.__ithRestoreBak;delete globalThis.__ithRestoreBak");
                let restore = inspector::restore_expr(av.as_deref(), cache.get("state"));
                let _ = inspector::eval_page(&conn, &restore);
            }
            inspector::detach(&conn);
            0
        }
        Err(e) => {
            eprintln!("attach: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 🔴-2 回归：详情序列化后按字节切，byte 120 落进 CJK 字符内部必 panic
    #[test]
    fn brief_truncation_is_char_boundary_safe() {
        // 构造 byte 120 恰在 "中" 字符内部："{\"sub\":{\"zh\":\"" 占 13B，
        // 其后每 "中" 3B——(120-13)%3=2 → 命中字符内部（老代码在此必 panic）
        let d = json!({"sub": {"zh": "中".repeat(150), "en": "reset soon"}});
        let b = brief120(&d);
        assert_eq!(b.chars().count(), 120);
        assert!(serde_json::to_string(&d).unwrap().len() > 120);
    }
}
