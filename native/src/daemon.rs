// is-tibo-happy 主控：启动宿主 → CDP 注入 widget → 抓数据推状态 → 断线自愈。
//   is-tibo-happy            常驻守护（LaunchAgent 语境）
//   --once    注入+推一次状态后退出
//   --no-quit App 已运行但没开调试端口时，不自动重启它，直接报错退出
//   --launch  手动授权：App 没在跑也拉起
//   --no-update 关闭每日热更新检查
// 资源纪律：只在 App 活着时工作；用户退出 App 后原地等待（30s 轮询），不主动拉起。
use crate::cdp::{self, Cdp, CdpEvent};
use crate::net::fetch_forecast;
use crate::state::{derive, resolve_display, sub_line};
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use wait_timeout::ChildExt;

const PORT: u16 = cdp::PORT;
const UA: &str = "is-tibo-happy/0.2 (+https://github.com/elijah7x/is-tibo-happy)"; // 自报家门：让源站能认出、限流、联系我们
const POLL: Duration = Duration::from_secs(15 * 60);  // 上游数据轮询
const RECONNECT: Duration = Duration::from_secs(3);   // CDP 断开后重试间隔
const APP_POLL: Duration = Duration::from_secs(30);   // App 不在时的等待轮询
const RELAUNCH_CAP: usize = 3;                        // 每小时最多重启 App 次数（防打架循环）
const RELAUNCH_WINDOW: Duration = Duration::from_secs(3600);

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
macro_rules! log {
    ($($a:tt)*) => { println!("[is-tibo-happy] {}", format!($($a)*)) };
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

// execSync 等价物：跑子进程并拿 stdout，超时就杀（osascript 挂起不能冻住主循环）
fn run_cmd(prog: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
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
    ["/Applications/ChatGPT.app".to_string(),
     format!("{}/Applications/ChatGPT.app", std::env::var("HOME").unwrap_or_default())]
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
            // pid 可能被无关进程复用 → 校验进程身份确实是本守护进程
            let ours = run_cmd("ps", &["-p", &pid.to_string(), "-o", "args="], Duration::from_secs(5))
                .map(|o| o.contains("is-tibo-happy"))
                .unwrap_or(false);
            if ours {
                eprintln!("[is-tibo-happy] already running (pid {pid})");
                std::process::exit(1);
            }
        }
    }
    let _ = std::fs::write(&pidfile, std::process::id().to_string());
    // 退出时清锁（atexit 语义）：用一个全局路径静态量注册
    struct Guard(PathBuf);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    Box::leak(Box::new(Guard(pidfile))); // 进程退出时由 Drop 清理（Rust 无 atexit，泄漏到进程末尾即可）
}

fn port_up() -> bool {
    cdp::get_version(PORT).is_ok()
}

fn app_running(exe: &Path) -> bool {
    run_cmd("pgrep", &["-f", &exe.to_string_lossy()], Duration::from_secs(5))
        .map(|o| !o.trim().is_empty())
        .unwrap_or(false)
}

fn spawn_app(exe: &Path) -> bool {
    log!("launching with debug port {PORT}");
    let _ = Command::new(exe)
        .arg(format!("--remote-debugging-port={PORT}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    for _ in 0..45 {
        thread::sleep(Duration::from_secs(1));
        if port_up() {
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
        self.times.retain(|t| now.duration_since(*t) < RELAUNCH_WINDOW);
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
    last_pushed: Mutex<String>,
    last_fetch_at: Mutex<i64>,
    last_fetch_ok: AtomicBool,
    last_err: Mutex<String>,
    cache: Mutex<Value>,     // {state, at} 或 Null
    cache_file: PathBuf,
    first_flag: PathBuf,
    tz: Option<String>,
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
            resolve_display(fresh.as_ref(), if c.is_null() { None } else { Some(&c) }, now_ms(), self.tz.as_deref())
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

    fn push_inner(&self) {
        if self.active.lock().unwrap().is_none() {
            return; // 断开期间不抓远端
        }
        let s = self.fetch_state();
        // 内容指纹：只有用户可见信息变化才重推
        let d = &s["detail"];
        let key = json!({
            "k": s["kind"], "sub": d["sub"],
            "d": d["daysSince"].as_f64().map(|x| x.floor()),
            "s": d["scheduledISO"], "w": d["windowEnd"], "t": d["targetStart"],
        })
        .to_string();
        {
            let mut lp = self.last_pushed.lock().unwrap();
            if *lp == key {
                return;
            }
            *lp = key;
        }
        let via = d.get("via").and_then(|v| v.as_str()).unwrap_or("-");
        let brief = serde_json::to_string(d).unwrap_or_default();
        log!("state: {} via={via} {}", s["kind"].as_str().unwrap_or("?"), &brief[..brief.len().min(120)]);
        let payload = format!("window.__ith && window.__ith.setState({})", s);
        if let Err(e) = self.eval(&payload) {
            log!("setState failed: {e}");
        }
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
            let _ = cdp::eval_js(cdp_conn, &format!("window.__ith && window.__ith.setState({state})"));
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
                        && x["url"].as_str().is_some_and(|u| u.starts_with("app://-/index.html"))
                        && !x["url"].as_str().is_some_and(|u| u.contains("initialRoute="))
                }) {
                    return Ok(t);
                }
            }
            thread::sleep(Duration::from_secs(1));
        }
        Err("main window target never appeared".into())
    }

    // 返回 true = 调试端口可用。launch_if_absent=false 时：App 没在跑就原地等待（不拽起来）。
    fn ensure_app(&self, launch_if_absent: bool) -> Result<bool, String> {
        if port_up() {
            return Ok(true);
        }
        if app_running(&self.exe) {
            // 在跑但没开调试端口 → 需要重启带端口；限速防反复打架
            if self.no_quit {
                return Err("app is running without debug port; quit it or drop --no-quit".into());
            }
            if !self.cap.lock().unwrap().allowed() {
                return Ok(false);
            }
            log!("app running without debug port; restarting it once");
            let _ = run_cmd("osascript", &["-e", "quit app \"ChatGPT\""], Duration::from_secs(5));
            for _ in 0..15 {
                if !app_running(&self.exe) {
                    break;
                }
                thread::sleep(Duration::from_secs(1));
            }
            return Ok(spawn_app(&self.exe));
        }
        if !launch_if_absent || !self.cap.lock().unwrap().allowed() {
            return Ok(false);
        }
        Ok(spawn_app(&self.exe))
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
        conn.send("Runtime.addBinding", json!({"name": "ithRefresh"}), Duration::from_secs(10))?;
        self.inject(&conn)?;

        if self.once {
            thread::sleep(Duration::from_millis(500));
            let _ = cdp::eval_js(&conn, "window.__ith && window.__ith.destroy()");
            return Ok(());
        }
        loop {
            match conn.recv_event(Duration::from_secs(1)) {
                Ok(CdpEvent::Message { method, params }) => match method.as_str() {
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
                },
                Ok(CdpEvent::Closed) => break,
                Err(mpsc_timeout) => {
                    let _ = mpsc_timeout;
                    if self.stopping.load(Ordering::Relaxed) {
                        // 优雅退出：摘掉 widget 再断
                        let _ = cdp::eval_js(&conn, "window.__ith && window.__ith.destroy()");
                        conn.close();
                        std::process::exit(0);
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
            // 分段 sleep 好让 stopping 快速生效
            let mut left = wait;
            while left > Duration::ZERO && !self.stopping.load(Ordering::Relaxed) {
                let step = left.min(Duration::from_secs(1));
                thread::sleep(step);
                left -= step;
            }
        }
    }

    fn exe_dir_marker(&self) -> PathBuf {
        exe_dir()
    }
}

pub fn run(args: &[String]) -> i32 {
    let once = args.iter().any(|a| a == "--once");
    let no_quit = args.iter().any(|a| a == "--no-quit");
    let force_launch = args.iter().any(|a| a == "--launch");
    let no_update = args.iter().any(|a| a == "--no-update") || std::env::var_os("ITH_NO_UPDATE").is_some();

    let dir = exe_dir();
    let logfile = dir.join("daemon.log");
    // 日志防膨胀：>1MB 截断重开
    if std::fs::metadata(&logfile).map(|m| m.len() > 1024 * 1024).unwrap_or(false) {
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
        cap: Mutex::new(RelaunchCap { times: Vec::new(), logged: false }),
        active: Arc::new(Mutex::new(None)),
        injected: AtomicBool::new(false),
        pushing: AtomicBool::new(false),
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
    let d = daemon.clone();
    while !stopping.load(Ordering::Relaxed) {
        // 仅"安装后首跑"（标记文件存在）或 --launch 时允许拉起 App；之后用户退出就只等不拉。
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
            thread::sleep(APP_POLL);
            continue;
        }
        let _ = std::fs::remove_file(&d.first_flag); // 一次性授权已消费（不等 session，防残留循环拉起）
        match d.session() {
            Ok(()) => session_fails = 0, // 正常断线：不算契约失效
            Err(e) => {
                session_fails += 1;
                // 连续建不起会话（端口被占/宿主改版/注入失败）→ 指数退避，3s 起封顶 15min
                let delay = (RECONNECT * 2u32.saturating_pow(session_fails - 1)).min(Duration::from_secs(15 * 60));
                log!("session failed (x{session_fails}): {e} — retry in {}s", delay.as_secs());
                if once || stopping.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(delay);
                continue;
            }
        }
        if once || stopping.load(Ordering::Relaxed) {
            break;
        }
        thread::sleep(RECONNECT);
    }
    0
}
