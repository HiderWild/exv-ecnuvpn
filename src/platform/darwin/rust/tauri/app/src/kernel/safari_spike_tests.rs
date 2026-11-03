//! safaridriver 可行性 spike（计划 B0）的真机验证（仅 darwin）。
//!
//! 跑法（显式开关 + `--ignored`，不进默认测试集）：
//!
//! ```text
//! EXV_DARWIN_NATIVE_SAFARI=1 CARGO_TARGET_DIR=<主仓>/target cargo test --locked \
//!   -p exv-vpn-darwin-tauri --bin exv-vpn-darwin-tauri native_safari -- --ignored --nocapture
//! ```
//!
//! 边界：只触本地回环（std TCP fixture 服务器 + safaridriver REST），不连真实网关与
//! 外网；Safari 窗口可能短暂出现（这正是被验证的行为）。所有外部进程
//! （safaridriver / Safari / 本地服务器）在收尾清理。
//!
//! 五项验证（对应计划 B0 节）：
//! ① 会话创建与导航（默认形态 + `safari:cleanSession` 形态分别试）；
//! ② W3C `GET /cookie` 能否读回 **HttpOnly** cookie（aTrust 会话提取前提，读不到=no-go）；
//! ③ `executeScript` 页面观察与表单预填（导航完成后执行）；
//! ④ 「允许远程自动化」开启状态检测、未开启错误形态、`safaridriver --enable` 授权 UX；
//! ⑤ 会话与用户真实 Safari 的关系（冷启动/附着/会话删除影响/cookie 跨次留存/痕迹位置）。
//!
//! 全部用例共享一把进程内互斥锁：Safari 同一时刻只能承载一个 WebDriver 会话
//! （man safaridriver NOTES），测试必须串行。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};

/// 串行守卫：同一时刻只允许一个用例驱动 Safari（单会话限制）。
static SPIKE_GUARD: OnceLock<Mutex<()>> = OnceLock::new();

fn spike_guard() -> &'static Mutex<()> {
    SPIKE_GUARD.get_or_init(|| Mutex::new(()))
}

fn gate() -> bool {
    if std::env::var("EXV_DARWIN_NATIVE_SAFARI").as_deref() == Ok("1") {
        return true;
    }
    println!("未设置 EXV_DARWIN_NATIVE_SAFARI=1，跳过（safaridriver 真机用例保护开关）");
    false
}

fn banner(title: &str) {
    println!("\n==================== {title} ====================");
}

/// 解析 HTTP/1.1 响应（含 chunked 退路；每次请求 `Connection: close`，读到 EOF 为止）。
fn parse_response(raw: &[u8]) -> Result<(u16, String), String> {
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| "响应缺少头部终止符".to_owned())?;
    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("状态行异常：{status_line:?}"))?;
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            content_length = rest.trim().parse().ok();
        }
        if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            chunked = true;
        }
    }
    let mut body = raw[head_end + 4..].to_vec();
    if chunked {
        body = decode_chunked(&body)?;
    } else if let Some(len) = content_length {
        body.truncate(len);
    }
    Ok((status, String::from_utf8_lossy(&body).to_string()))
}

fn decode_chunked(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    loop {
        let line_end = data[i..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .map(|p| i + p)
            .ok_or_else(|| "chunked 长度行不完整".to_owned())?;
        let size_token = String::from_utf8_lossy(&data[i..line_end]);
        let size_hex = size_token.trim().split(';').next().unwrap_or("0");
        let size = usize::from_str_radix(size_hex, 16).map_err(|e| format!("chunk 尺寸解析失败：{e}"))?;
        i = line_end + 2;
        if size == 0 {
            break;
        }
        if i + size > data.len() {
            return Err("chunked 体不完整".to_owned());
        }
        out.extend_from_slice(&data[i..i + size]);
        i += size + 2;
    }
    Ok(out)
}

/// 最小 W3C WebDriver HTTP 客户端（每次请求新建连接）。
fn http_request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    read_timeout: Duration,
) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("连接失败：{e}"))?;
    stream
        .set_read_timeout(Some(read_timeout))
        .map_err(|e| format!("设置读超时失败：{e}"))?;
    let body_bytes = body.map(str::as_bytes).unwrap_or_default();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
         Content-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body_bytes.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|_| stream.write_all(body_bytes))
        .and_then(|_| stream.flush())
        .map_err(|e| format!("发送失败：{e}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("读取失败：{e}"))?;
    parse_response(&raw)
}

/// 本进程管理的 safaridriver 子进程（Drop 时无条件回收）。
struct Driver {
    child: Child,
    port: u16,
    log: Arc<Mutex<String>>,
}

fn free_port() -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("绑定失败：{e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("local_addr 失败：{e}"))?
        .port();
    drop(listener);
    Ok(port)
}

impl Driver {
    fn start() -> Result<Driver, String> {
        let port = free_port()?;
        let mut child = Command::new("/usr/bin/safaridriver")
            .arg("--port")
            .arg(port.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("启动 safaridriver 失败：{e}"))?;
        let log = Arc::new(Mutex::new(String::new()));
        let pipes: Vec<Box<dyn Read + Send>> = vec![
            child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>),
            child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        .collect();
        for mut pipe in pipes {
            let log = Arc::clone(&log);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match pipe.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            log.lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .push_str(&String::from_utf8_lossy(&buf[..n]));
                        }
                    }
                }
            });
        }
        for _ in 0..80 {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return Ok(Driver { child, port, log });
            }
            if let Ok(Some(_)) = child.try_wait() {
                let captured = log
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                return Err(format!("safaridriver 提前退出，输出：\n{captured}"));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let _ = child.kill();
        let _ = child.wait();
        Err("等待 safaridriver 端口就绪超时".to_owned())
    }

    fn wd(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        read_timeout: Duration,
    ) -> Result<(u16, String), String> {
        http_request(self.port, method, path, body, read_timeout)
    }

    fn print_log(&self, label: &str) {
        let captured = self
            .log
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if captured.trim().is_empty() {
            println!("[safaridriver {label}]（无输出）");
        } else {
            println!("[safaridriver {label}] 输出：\n{captured}");
        }
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 本地 fixture 服务器：不触真实网关与外网。
struct FixtureServer {
    port: u16,
    stop: Arc<std::sync::atomic::AtomicBool>,
    log: Arc<Mutex<Vec<String>>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

const HTTP_ONLY_COOKIE: &str = "spike_http_only=sid-test-123";
const VISIBLE_COOKIE: &str = "spike_visible=plain-456";

fn html_page(body: &str) -> String {
    format!("<!doctype html><html><head><title>exv-safari-spike</title></head><body>{body}</body></html>")
}

fn html_escape(raw: &str) -> String {
    raw.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

impl FixtureServer {
    fn start() -> FixtureServer {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("绑定 fixture 端口");
        let port = listener.local_addr().expect("读取 fixture 端口").port();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let stop_for_thread = Arc::clone(&stop);
        let log_for_thread = Arc::clone(&log);
        let handle = std::thread::spawn(move || {
            while !stop_for_thread.load(std::sync::atomic::Ordering::SeqCst) {
                let Ok((mut stream, _peer)) = listener.accept() else {
                    break;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = Vec::new();
                let mut chunk = [0u8; 2048];
                // 读到头部终止符（或超时/对端关闭）为止
                loop {
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16384 {
                        break;
                    }
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head_end = buf
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .unwrap_or(buf.len());
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let mut lines = head.split("\r\n");
                let request_line = lines.next().unwrap_or_default().to_owned();
                let mut content_length = 0usize;
                let mut cookie_header = String::new();
                for line in lines {
                    let lower = line.to_ascii_lowercase();
                    if let Some(rest) = lower.strip_prefix("content-length:") {
                        content_length = rest.trim().parse().unwrap_or(0);
                    }
                    if lower.starts_with("cookie:") {
                        cookie_header = line[":cookie:".len()..].trim().to_owned();
                    }
                }
                let mut body = buf[head_end.min(buf.len())..].to_vec();
                while body.len() < content_length {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => body.extend_from_slice(&chunk[..n]),
                    }
                }
                let path = request_line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let path_only = path.split('?').next().unwrap_or("/").to_owned();
                let (status, headers, page) = match path_only.as_str() {
                    "/issue" => (
                        "200 OK",
                        format!(
                            "Set-Cookie: {HTTP_ONLY_COOKIE}; Path=/; HttpOnly\r\n\
                             Set-Cookie: {VISIBLE_COOKIE}; Path=/\r\n"
                        ),
                        html_page(
                            "<span id=\"marker\">fixture-issue</span><pre id=\"server-cookie\">issued</pre>",
                        ),
                    ),
                    "/form" => (
                        "200 OK",
                        String::new(),
                        html_page(
                            "<span id=\"marker\">fixture-marker</span><input id=\"username\" type=\"text\">",
                        ),
                    ),
                    "/reflect" => (
                        "200 OK",
                        String::new(),
                        html_page(&format!(
                            "<pre id=\"server-cookie\">{}</pre>",
                            html_escape(&cookie_header)
                        )),
                    ),
                    _ => ("404 Not Found", String::new(), "not found".to_owned()),
                };
                log_for_thread
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(format!(
                        "{request_line} Cookie[{cookie_header}] body_len={}",
                        body.len()
                    ));
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
                     {headers}Content-Length: {}\r\nConnection: close\r\n\r\n{page}",
                    page.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        FixtureServer {
            port,
            stop,
            log,
            handle: Some(handle),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn log_snapshot(&self) -> Vec<String> {
        self.log
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn print_log(&self, label: &str) {
        println!("---- fixture 服务器请求记录（{label}）----");
        for entry in self.log_snapshot() {
            println!("  {entry}");
        }
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        // 唤醒阻塞在 accept 上的线程后回收
        if let Ok(stream) = TcpStream::connect(("127.0.0.1", self.port)) {
            drop(stream);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

// ---------- W3C 命令封装 ----------

// 冷启动 Safari 的首次会话建立实测可超 90 秒（读超时以 EAGAIN 形态返回），放宽并带一次重试。
const SESSION_CREATION_TIMEOUT: Duration = Duration::from_secs(150);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

fn new_session(
    driver: &Driver,
    clean_session: Option<bool>,
) -> Result<(String, Value), (u16, String)> {
    let capabilities = match clean_session {
        None => json!({"capabilities": {"alwaysMatch": {"browserName": "safari"}}}),
        Some(v) => json!({
            "capabilities": {"alwaysMatch": {
                "browserName": "safari",
                "safari:cleanSession": v
            }}
        }),
    };
    let mut last_transport_error = String::new();
    let mut response = None;
    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(2));
        }
        match driver.wd(
            "POST",
            "/session",
            Some(&capabilities.to_string()),
            SESSION_CREATION_TIMEOUT,
        ) {
            Ok(result) => {
                response = Some(result);
                break;
            }
            Err(error) => {
                println!("会话建立传输失败（第 {} 次）：{error}", attempt + 1);
                last_transport_error = error;
            }
        }
    }
    let (status, body) = response.ok_or_else(|| (0u16, last_transport_error))?;
    if status != 200 {
        return Err((status, body));
    }
    let parsed: Value = serde_json::from_str(&body).unwrap_or_else(|_| json!({"raw": body}));
    let session_id = parsed["value"]["sessionId"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| (status, body.clone()))?;
    Ok((session_id, parsed["value"]["capabilities"].clone()))
}

fn navigate(driver: &Driver, session: &str, url: &str) -> Result<(), (u16, String)> {
    let body = json!({"url": url}).to_string();
    let (status, response) = driver
        .wd("POST", &format!("/session/{session}/url"), Some(&body), COMMAND_TIMEOUT)
        .map_err(|e| (0u16, e))?;
    if status == 200 {
        Ok(())
    } else {
        Err((status, response))
    }
}

fn current_url(driver: &Driver, session: &str) -> String {
    driver
        .wd("GET", &format!("/session/{session}/url"), None, COMMAND_TIMEOUT)
        .map(|(_, body)| body)
        .unwrap_or_default()
}

fn get_cookies(driver: &Driver, session: &str) -> Value {
    driver
        .wd("GET", &format!("/session/{session}/cookie"), None, COMMAND_TIMEOUT)
        .map(|(_, body)| serde_json::from_str(&body).unwrap_or(Value::Null))
        .unwrap_or(Value::Null)
}

fn execute_script(
    driver: &Driver,
    session: &str,
    script: &str,
    args: Value,
) -> Result<Value, (u16, String)> {
    // Safari 26 实测：W3C REC 命名 /execute/sync-script 与旧 /execute 均 404；
    // safaridriver 采用 Apple 文档（macOS WebDriver Commands for Safari 12 and later）
    // 列出的 /execute/sync。
    let body = json!({"script": script, "args": args}).to_string();
    let (status, response) = driver
        .wd(
            "POST",
            &format!("/session/{session}/execute/sync"),
            Some(&body),
            COMMAND_TIMEOUT,
        )
        .map_err(|e| (0u16, e))?;
    if status == 200 {
        let parsed: Value = serde_json::from_str(&response).unwrap_or(Value::Null);
        Ok(parsed["value"].clone())
    } else {
        Err((status, response))
    }
}

fn window_handles(driver: &Driver, session: &str) -> Value {
    driver
        .wd("GET", &format!("/session/{session}/window/handles"), None, COMMAND_TIMEOUT)
        .map(|(_, body)| serde_json::from_str(&body).unwrap_or(Value::Null))
        .unwrap_or(Value::Null)["value"]
        .clone()
}

fn delete_session(driver: &Driver, session: &str) {
    let _ = driver.wd("DELETE", &format!("/session/{session}"), None, COMMAND_TIMEOUT);
}

// ---------- Safari 进程面 ----------

fn safari_running() -> bool {
    Command::new("pgrep")
        .arg("-x")
        .arg("Safari")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn open_safari() {
    let _ = Command::new("open")
        .args(["-a", "Safari"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn quit_safari() {
    let _ = Command::new("pkill")
        .args(["-x", "Safari"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn wait_until(label: &str, seconds: u64, predicate: impl Fn() -> bool) -> bool {
    for _ in 0..(seconds * 4) {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    println!("等待超时：{label}");
    false
}

/// 收尾纪律：仅当本用例启动了 Safari（进入时未运行）才负责退出它。
struct SafariCleanup {
    was_running_at_start: bool,
}

impl SafariCleanup {
    fn enter() -> SafariCleanup {
        SafariCleanup {
            was_running_at_start: safari_running(),
        }
    }
}

impl Drop for SafariCleanup {
    fn drop(&mut self) {
        if !self.was_running_at_start && safari_running() {
            quit_safari();
            let _ = wait_until("Safari 退出", 10, || !safari_running());
        }
    }
}

fn cookie_entries(cookies: Value) -> Vec<Value> {
    cookies
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn find_cookie<'a>(cookies: &'a [Value], name: &str) -> Option<&'a Value> {
    cookies.iter().find(|cookie| cookie["name"].as_str() == Some(name))
}

fn profile_evidence(label: &str) {
    println!("---- Safari 状态文件快照（{label}）----");
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for path in [
            home.join("Library/Containers/com.apple.Safari"),
            home.join("Library/Containers/com.apple.Safari/Data"),
            home.join("Library/Safari"),
            home.join("Library/Cookies/Cookies.binarycookies"),
        ] {
            match std::fs::metadata(&path) {
                Ok(meta) => {
                    let mtime = meta
                        .modified()
                        .map(|t| format!("{t:?}"))
                        .unwrap_or_else(|e| format!("读取失败({e})"));
                    println!("  {} 存在 len={} mtime={mtime}", path.display(), meta.len());
                }
                Err(err) => println!("  {} 不可读/不存在（{err}）", path.display()),
            }
        }
    }
}

/// ① 会话创建与导航：默认形态 + `safari:cleanSession` 形态分别建会话并导航本地 fixture。
#[test]
#[ignore = "真机 safaridriver；EXV_DARWIN_NATIVE_SAFARI=1 显式启用"]
fn native_safari_1_session_creation_and_navigation_local_fixture() {
    if !gate() {
        return;
    }
    let _guard = spike_guard().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _safari_cleanup = SafariCleanup::enter();
    let server = FixtureServer::start();

    banner("① 形态一：默认 capabilities（无 safari:cleanSession）");
    let driver = Driver::start().expect("启动 safaridriver");
    let (session, capabilities) = new_session(&driver, None).expect("默认形态创建会话");
    println!("sessionId={session}");
    println!("capabilities 回显：{capabilities}");
    navigate(&driver, &session, &server.url("/form")).expect("导航到本地 fixture 页");
    println!("current_url={}", current_url(&driver, &session));
    assert!(
        server.log_snapshot().iter().any(|entry| entry.contains("GET /form")),
        "fixture 服务器必须观察到真实导航请求"
    );
    delete_session(&driver, &session);
    driver.print_log("① 形态一结束");
    drop(driver);

    banner("① 形态二：safari:cleanSession = true");
    let driver = Driver::start().expect("启动 safaridriver（cleanSession 形态）");
    match new_session(&driver, Some(true)) {
        Ok((session, capabilities)) => {
            println!("cleanSession 会话创建成功，capabilities 回显：{capabilities}");
            navigate(&driver, &session, &server.url("/form")).expect("cleanSession 会话导航");
            println!("current_url={}", current_url(&driver, &session));
            delete_session(&driver, &session);
        }
        Err((status, body)) => {
            println!("cleanSession 会话创建失败（status={status}）：\n{body}");
            println!("（形态本身按实测记录，不判失败——Safari 26 手册已不再列举该能力）");
        }
    }
    server.print_log("① 结束");
}

/// ② HttpOnly cookie 读回：fixture 经 Set-Cookie（HttpOnly; Path=/）下发后 W3C GET /cookie 断言。
#[test]
#[ignore = "真机 safaridriver；EXV_DARWIN_NATIVE_SAFARI=1 显式启用"]
fn native_safari_2_reads_http_only_cookie_via_w3c_get_cookie() {
    if !gate() {
        return;
    }
    let _guard = spike_guard().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _safari_cleanup = SafariCleanup::enter();
    let server = FixtureServer::start();

    let driver = Driver::start().expect("启动 safaridriver");
    let (session, _) = new_session(&driver, None).expect("创建会话");
    navigate(&driver, &session, &server.url("/issue")).expect("导航到下发页");
    assert!(
        server
            .log_snapshot()
            .iter()
            .any(|entry| entry.contains("GET /issue")),
        "fixture 服务器必须观察到 Set-Cookie 下发请求"
    );
    let cookies = get_cookies(&driver, &session);
    println!("GET /cookie 原始返回：{cookies}");
    let entries = cookie_entries(cookies["value"].clone());
    let http_only = find_cookie(&entries, "spike_http_only")
        .unwrap_or_else(|| panic!("必须能读到 HttpOnly cookie spike_http_only，实际：{entries:?}"));
    assert_eq!(http_only["value"].as_str(), Some("sid-test-123"));
    assert_eq!(
        http_only["httpOnly"].as_bool(),
        Some(true),
        "spike_http_only 必须以 httpOnly=true 形态读回"
    );
    if let Some(visible) = find_cookie(&entries, "spike_visible") {
        println!("对照非 HttpOnly cookie：{visible}");
        assert_ne!(visible["httpOnly"].as_bool(), Some(true));
    }
    delete_session(&driver, &session);
    server.print_log("② 结束");
}

/// ③ executeScript：导航完成后读取 DOM 状态与表单预填（赋值后读回）。
#[test]
#[ignore = "真机 safaridriver；EXV_DARWIN_NATIVE_SAFARI=1 显式启用"]
fn native_safari_3_execute_script_observes_dom_and_fills_input() {
    if !gate() {
        return;
    }
    let _guard = spike_guard().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _safari_cleanup = SafariCleanup::enter();
    let server = FixtureServer::start();

    let driver = Driver::start().expect("启动 safaridriver");
    let (session, _) = new_session(&driver, None).expect("创建会话");
    navigate(&driver, &session, &server.url("/form")).expect("导航到表单页");

    // Safari 26 实测语义：脚本按**函数体**执行——裸表达式返回 null，必须显式 return；
    // 参数以 JSON Wire 风格经 arguments[0..n] 注入（W3C「可调用即调用」语义不生效）。
    let ready_state = execute_script(&driver, &session, "return document.readyState;", json!([]))
        .expect("读取 readyState");
    println!("导航命令返回后 readyState={ready_state}");
    assert_eq!(ready_state.as_str(), Some("complete"));

    let marker = execute_script(
        &driver,
        &session,
        "return document.getElementById('marker').textContent;",
        json!([]),
    )
    .expect("读取 DOM 状态");
    println!("DOM 观察 marker={marker}");
    assert_eq!(marker.as_str(), Some("fixture-marker"));

    let filled = execute_script(
        &driver,
        &session,
        "const input = document.getElementById('username'); \
         input.value = arguments[0]; return input.value;",
        json!(["exv-spike-user"]),
    )
    .expect("表单预填并读回");
    println!("表单预填读回={filled}");
    assert_eq!(filled.as_str(), Some("exv-spike-user"));

    delete_session(&driver, &session);
    server.print_log("③ 结束");
}

/// ④ 「允许远程自动化」状态检测、未开启错误形态、`safaridriver --enable` 授权 UX。
#[test]
#[ignore = "真机 safaridriver；EXV_DARWIN_NATIVE_SAFARI=1 显式启用"]
fn native_safari_4_enablement_state_and_enable_flag_ux() {
    if !gate() {
        return;
    }
    let _guard = spike_guard().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _safari_cleanup = SafariCleanup::enter();

    banner("④ 探测：会话建立（最轻量判法）");
    let driver = Driver::start().expect("启动 safaridriver");
    match new_session(&driver, None) {
        Ok((session, _)) => {
            println!("判定：允许远程自动化**已开启**（判法：safaridriver 会话建立探测成功）");
            delete_session(&driver, &session);
            driver.print_log("④ 已开启分支");
        }
        Err((status, body)) => {
            println!("判定：未开启。会话建立失败（status={status}）原始返回：\n{body}");
            driver.print_log("④ 未开启分支");
            drop(driver);
            banner("④ 尝试 safaridriver --enable（stdin 关闭，记录授权 UX）");
            let mut enable = Command::new("/usr/bin/safaridriver")
                .arg("--enable")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("启动 safaridriver --enable");
            let mut exited = false;
            for _ in 0..120 {
                if let Ok(Some(_)) = enable.try_wait() {
                    exited = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            if !exited {
                let _ = enable.kill();
                println!("--enable 120 秒未退出（等待授权输入），已终止");
            }
            let output = enable.wait_with_output().expect("回收 --enable");
            println!(
                "--enable 退出码={} stdout={} stderr={}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            banner("④ --enable 后复测会话建立");
            let driver = Driver::start().expect("启动 safaridriver（复测）");
            match new_session(&driver, None) {
                Ok((session, _)) => {
                    println!("--enable 后会话建立成功（授权已生效）");
                    delete_session(&driver, &session);
                }
                Err((status, body)) => {
                    println!("--enable 后仍失败（status={status}）：\n{body}");
                }
            }
        }
    }
}

/// ⑤ 会话与用户真实 Safari 的关系：冷启动、附着、会话删除影响、cookie 跨次留存与痕迹。
#[test]
#[ignore = "真机 safaridriver；EXV_DARWIN_NATIVE_SAFARI=1 显式启用"]
fn native_safari_5_session_relationship_with_real_safari() {
    if !gate() {
        return;
    }
    let _guard = spike_guard().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _safari_cleanup = SafariCleanup::enter();
    let server = FixtureServer::start();
    profile_evidence("⑤ 开始前");

    banner("⑤-A Safari 未运行时新建会话");
    if safari_running() {
        println!("测试进入时 Safari 已在运行，⑤-A 冷启动观察按现状记录（跳过冷启动前置）");
    }
    wait_until("Safari 完全退出", 10, || !safari_running());
    let driver = Driver::start().expect("启动 safaridriver（⑤-A）");
    let (session_a, _) = new_session(&driver, None).expect("⑤-A 创建会话");
    let safari_launched_by_session = wait_until("会话启动 Safari", 20, safari_running);
    println!(
        "会话创建后 Safari 进程{}",
        if safari_launched_by_session { "已出现（用户可见 Safari 被启动）" } else { "未出现" }
    );
    let handles_a = window_handles(&driver, &session_a);
    println!("⑤-A 会话窗口句柄：{handles_a}");
    navigate(&driver, &session_a, &server.url("/form")).expect("⑤-A 导航");
    delete_session(&driver, &session_a);
    drop(driver);
    std::thread::sleep(Duration::from_secs(3));
    let safari_alive_after_delete = safari_running();
    println!(
        "会话删除+driver 退出后 Safari 进程{}",
        if safari_alive_after_delete { "仍在运行" } else { "已随之退出" }
    );
    if safari_alive_after_delete {
        quit_safari();
        wait_until("⑤-A 收尾 Safari 退出", 10, || !safari_running());
    }

    banner("⑤-B Safari 已运行时会话附着还是新起窗口");
    // 刚被退出的 Safari 立即 relaunch 可能撞上 LaunchServices 节流（实测偶发 >20s），
    // 用「重发 open + 轮询」扛过该窗口。
    let mut safari_opened = false;
    for _ in 0..15 {
        if safari_running() {
            safari_opened = true;
            break;
        }
        open_safari();
        std::thread::sleep(Duration::from_secs(2));
    }
    assert!(safari_opened, "open -a 必须能启动 Safari");
    std::thread::sleep(Duration::from_secs(3));
    let driver = Driver::start().expect("启动 safaridriver（⑤-B）");
    let (session_b, _) = new_session(&driver, None).expect("⑤-B 创建会话");
    let handles_b1 = window_handles(&driver, &session_b);
    let count_b1 = handles_b1.as_array().map(Vec::len).unwrap_or(0);
    println!(
        "⑤-B 会话建立后窗口句柄（共 {count_b1} 个）：{handles_b1}——{}",
        if count_b1 >= 2 { "新起自动化窗口" } else { "接管既有窗口" }
    );
    navigate(&driver, &session_b, &server.url("/reflect")).expect("⑤-B 导航");
    let handles_b2 = window_handles(&driver, &session_b);
    println!("⑤-B 导航后窗口句柄：{handles_b2}");
    delete_session(&driver, &session_b);
    drop(driver);
    std::thread::sleep(Duration::from_secs(3));
    let safari_alive_after_b = safari_running();
    println!(
        "⑤-B 会话删除后 Safari 进程{}",
        if safari_alive_after_b { "仍在运行（用户窗口保留）" } else { "已退出" }
    );
    server.print_log("⑤-B 结束");
    if safari_alive_after_b {
        quit_safari();
        wait_until("⑤-B 收尾 Safari 退出", 10, || !safari_running());
    }

    banner("⑤-C1 cookie 跨会话留存（含 Safari 完全退出与 driver 进程重建）");
    let driver = Driver::start().expect("启动 safaridriver（⑤-C1 第一轮）");
    let (session_1, _) = new_session(&driver, None).expect("⑤-C1 第一轮会话");
    navigate(&driver, &session_1, &server.url("/issue")).expect("⑤-C1 下发 cookie");
    let cookies_1 = get_cookies(&driver, &session_1);
    let entries_1 = cookie_entries(cookies_1["value"].clone());
    assert!(
        find_cookie(&entries_1, "spike_http_only").is_some(),
        "⑤-C1 第一轮必须读到 spike_http_only"
    );
    delete_session(&driver, &session_1);
    drop(driver);
    // 显式退出 Safari（本用例启动的实例）：留存判定必须覆盖「Safari 完全重启」，
    // 不能只凭会话边界。
    if safari_running() {
        quit_safari();
    }
    assert!(
        wait_until("⑤-C1 中场 Safari 退出", 10, || !safari_running()),
        "必须确认 Safari 完全退出后再做第二轮判定"
    );

    let driver = Driver::start().expect("启动 safaridriver（⑤-C1 第二轮）");
    let (session_2, _) = new_session(&driver, None).expect("⑤-C1 第二轮会话");
    navigate(&driver, &session_2, &server.url("/reflect")).expect("⑤-C1 第二轮导航");
    let cookies_2 = get_cookies(&driver, &session_2);
    println!("⑤-C1 第二轮 GET /cookie：{cookies_2}");
    let entries_2 = cookie_entries(cookies_2["value"].clone());
    let persisted = find_cookie(&entries_2, "spike_http_only");
    println!(
        "⑤-C1 cookie 跨次留存判定：{}",
        if persisted.is_some() { "留存（写入用户真实 profile）" } else { "不留存（会话级隔离）" }
    );
    if let Some(cookie) = persisted {
        println!("留存形态：{cookie}");
    }
    let reflect_entries: Vec<String> = server
        .log_snapshot()
        .into_iter()
        .filter(|entry| entry.contains("GET /reflect"))
        .collect();
    println!("⑤-C1 /reflect 请求的浏览器侧 Cookie 头：");
    for entry in &reflect_entries {
        println!("  {entry}");
    }
    delete_session(&driver, &session_2);
    drop(driver);
    if safari_running() {
        quit_safari();
    }
    assert!(
        wait_until("⑤-C2 中场 Safari 退出", 10, || !safari_running()),
        "必须确认 Safari 完全退出后再做 cleanSession 判定"
    );

    banner("⑤-C2 safari:cleanSession 语义（隔离还是清除真实 profile）");
    let driver = Driver::start().expect("启动 safaridriver（⑤-C2 clean 形态）");
    match new_session(&driver, Some(true)) {
        Ok((session_3, capabilities)) => {
            println!("cleanSession 会话建立成功：{capabilities}");
            navigate(&driver, &session_3, &server.url("/reflect")).expect("⑤-C2 clean 导航");
            let cookies_3 = get_cookies(&driver, &session_3);
            println!("⑤-C2 clean 会话 GET /cookie：{cookies_3}");
            let entries_3 = cookie_entries(cookies_3["value"].clone());
            println!(
                "clean 会话{}既有留存 cookie",
                if find_cookie(&entries_3, "spike_http_only").is_some() { "仍能看到" } else { "看不到（干净起点）" }
            );
            delete_session(&driver, &session_3);
        }
        Err((status, body)) => {
            println!("cleanSession 会话建立失败（status={status}）：\n{body}");
        }
    }
    drop(driver);
    if safari_running() {
        quit_safari();
    }
    assert!(
        wait_until("⑤-C2 复测前 Safari 退出", 10, || !safari_running()),
        "必须确认 Safari 完全退出后再复测真实 profile 状态"
    );

    let driver = Driver::start().expect("启动 safaridriver（⑤-C2 复测形态）");
    let (session_4, _) = new_session(&driver, None).expect("⑤-C2 复测会话");
    navigate(&driver, &session_4, &server.url("/reflect")).expect("⑤-C2 复测导航");
    let cookies_4 = get_cookies(&driver, &session_4);
    let entries_4 = cookie_entries(cookies_4["value"].clone());
    println!(
        "clean 会话之后普通会话{}既有留存 cookie",
        if find_cookie(&entries_4, "spike_http_only").is_some() { "仍能看到（未清除真实 profile）" } else { "已看不到（真实 profile 被清除）" }
    );
    println!("⑤-C2 复测 GET /cookie：{cookies_4}");
    delete_session(&driver, &session_4);
    drop(driver);

    server.print_log("⑤ 结束");
    profile_evidence("⑤ 结束后");
}
