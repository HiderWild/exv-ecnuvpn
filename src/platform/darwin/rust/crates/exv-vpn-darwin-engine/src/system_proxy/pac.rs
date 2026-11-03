//! 本机 HTTP PAC 的单次包装；端点由现有代理 Owner 持有。

use super::Result;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const LIMIT: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(3);

fn source(url: &str) -> Result<(SocketAddrV4, String)> {
    let rest = url
        .strip_prefix("http://127.0.0.1:")
        .ok_or("pac_url_unsupported")?;
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let port = rest[..split].parse::<u16>().map_err(|_| "pac_url_port")?;
    if port == 0 || rest.bytes().any(|b| b <= 32 || b == 127) || rest.contains('#') {
        return Err("pac_url_invalid".into());
    }
    let path = match &rest[split..] {
        "" => "/".to_owned(),
        query if query.starts_with('?') => format!("/{query}"),
        path => path.to_owned(),
    };
    Ok((SocketAddrV4::new(Ipv4Addr::LOCALHOST, port), path))
}

pub(super) fn fetch(url: &str) -> Result<String> {
    let (address, path) = source(url)?;
    let deadline = Instant::now() + TIMEOUT;
    let mut stream =
        TcpStream::connect_timeout(&address.into(), TIMEOUT).map_err(|_| "pac_fetch_connect")?;
    stream
        .set_write_timeout(Some(TIMEOUT))
        .map_err(|_| "pac_fetch_timeout")?;
    let request = format!("GET {path} HTTP/1.0\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|_| "pac_fetch_write")?;
    let mut bytes = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("pac_fetch_timeout".into());
        }
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|_| "pac_fetch_timeout")?;
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk).map_err(|_| "pac_fetch_read")?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() > LIMIT {
            return Err("pac_fetch_too_large".into());
        }
    }
    let split = bytes
        .windows(4)
        .position(|b| b == b"\r\n\r\n")
        .ok_or("pac_fetch_header")?;
    let head = std::str::from_utf8(&bytes[..split]).map_err(|_| "pac_fetch_header")?;
    let mut lines = head.split("\r\n");
    let status = lines.next().ok_or("pac_fetch_status")?;
    if !matches!(
        status.split_whitespace().collect::<Vec<_>>().as_slice(),
        ["HTTP/1.0" | "HTTP/1.1", "200", ..]
    ) {
        return Err("pac_fetch_status".into());
    }
    let body = &bytes[split + 4..];
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("transfer-encoding") {
                return Err("pac_fetch_encoding_unsupported".into());
            }
            if key.eq_ignore_ascii_case("content-length")
                && value.trim().parse::<usize>().ok() != Some(body.len())
            {
                return Err("pac_fetch_truncated".into());
            }
        }
    }
    String::from_utf8(body.to_vec()).map_err(|_| "pac_fetch_utf8".into())
}

pub(super) fn wrap(original: &str, additions: &[String]) -> Result<String> {
    if !original.contains("function FindProxyForURL") || original.contains("__exv_pac_original") {
        return Err("pac_script_unwrappable".into());
    }
    let mut conditions = String::new();
    for entry in additions {
        if let Some((address, prefix)) = entry.split_once('/') {
            let address: Ipv4Addr = address.parse().map_err(|_| "pac_route_address")?;
            let prefix: u32 = prefix.parse().map_err(|_| "pac_route_prefix")?;
            if prefix > 32 {
                return Err("pac_route_prefix".into());
            }
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            conditions.push_str(&format!(
                "if (ip !== null && ((ip & {mask}) >>> 0) === {}) return 'DIRECT';\n",
                u32::from(address) & mask
            ));
        } else if let Some(domain) = entry.strip_prefix("*.") {
            let suffix = serde_json::to_string(&format!(".{domain}").to_ascii_lowercase())
                .map_err(|_| "pac_domain")?;
            conditions.push_str(&format!("if (h.length > {suffix}.length && h.slice(-{suffix}.length) === {suffix}) return 'DIRECT';\n"));
        } else {
            let domain =
                serde_json::to_string(&entry.to_ascii_lowercase()).map_err(|_| "pac_domain")?;
            conditions.push_str(&format!("if (h === {domain}) return 'DIRECT';\n"));
        }
    }
    // 赋值发生在原脚本执行之后，不用函数声明覆盖原声明，避免提升导致自递归。
    let result = format!(
        r#"{original}
;var __exv_pac_original = FindProxyForURL;
FindProxyForURL = function(url, host) {{
  var h = String(host || '').toLowerCase();
  var p = h.split('.'), ip = null;
  if (p.length === 4) {{
    ip = 0;
    for (var i = 0; i < 4; i++) {{
      if (!/^\d{{1,3}}$/.test(p[i]) || Number(p[i]) > 255) {{ ip = null; break; }}
      ip = ip * 256 + Number(p[i]);
    }}
  }}
  {conditions}
  return __exv_pac_original(url, host);
}};
"#
    );
    validate(&result)?;
    Ok(result)
}

/// 生产只做系统语法检查，不在 Engine 内执行用户 PAC；求值由系统 PAC 消费者负责。
#[cfg(target_os = "macos")]
fn validate(script: &str) -> Result<()> {
    javascript::check_syntax(script)
}
#[cfg(not(target_os = "macos"))]
fn validate(_: &str) -> Result<()> {
    Err("pac_requires_macos".into())
}

#[cfg(target_os = "macos")]
mod javascript {
    use super::Result;
    use std::ffi::{CString, c_char, c_void};
    type Ref = *const c_void;
    #[link(name = "JavaScriptCore", kind = "framework")]
    unsafe extern "C" {
        fn JSGlobalContextCreate(class: Ref) -> Ref;
        fn JSGlobalContextRelease(context: Ref);
        fn JSCheckScriptSyntax(
            context: Ref,
            script: Ref,
            url: Ref,
            line: i32,
            exception: *mut Ref,
        ) -> bool;
        fn JSStringCreateWithUTF8CString(text: *const c_char) -> Ref;
        fn JSStringRelease(text: Ref);
    }
    pub(super) fn check_syntax(script: &str) -> Result<()> {
        let script = CString::new(script).map_err(|_| "pac_script_nul")?;
        unsafe {
            let context = JSGlobalContextCreate(std::ptr::null());
            if context.is_null() {
                return Err("pac_js_context".into());
            }
            let text = JSStringCreateWithUTF8CString(script.as_ptr());
            let valid =
                JSCheckScriptSyntax(context, text, std::ptr::null(), 1, std::ptr::null_mut());
            JSStringRelease(text);
            JSGlobalContextRelease(context);
            if valid {
                Ok(())
            } else {
                Err("pac_script_invalid".into())
            }
        }
    }
}

/// TCP 不保证单次 read 得到完整首行；同时限制整份请求的字节和总等待。
fn read_request(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut bytes = Vec::new();
    while bytes.len() < 4096 {
        let budget = deadline.saturating_duration_since(Instant::now());
        if budget.is_zero() {
            return Err("pac_endpoint_request_timeout".into());
        }
        stream
            .set_read_timeout(Some(budget))
            .map_err(|_| "pac_endpoint_request_timeout")?;
        let mut chunk = [0; 512];
        let n = stream
            .read(&mut chunk)
            .map_err(|_| "pac_endpoint_request_read")?;
        if n == 0 {
            return Err("pac_endpoint_request_eof".into());
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.windows(4).any(|b| b == b"\r\n\r\n") {
            return Ok(bytes);
        }
    }
    Err("pac_endpoint_request_too_large".into())
}

pub(super) struct Endpoint {
    url: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}
impl Endpoint {
    pub(super) fn start(script: String) -> Result<Self> {
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| "pac_endpoint_bind")?;
        listener
            .set_nonblocking(true)
            .map_err(|_| "pac_endpoint_nonblocking")?;
        let url = format!(
            "http://127.0.0.1:{}/proxy.pac",
            listener
                .local_addr()
                .map_err(|_| "pac_endpoint_address")?
                .port()
        );
        let stop = Arc::new(AtomicBool::new(false));
        let closing = stop.clone();
        let handle = std::thread::Builder::new().name("exv-pac".into()).spawn(move || {
            while !closing.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Darwin accept 可能继承监听 socket 的非阻塞标志；请求读取使用有界阻塞。
                        if stream.set_nonblocking(false).is_err() { continue; }
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                        let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                        if let Ok(request) = read_request(&mut stream) {
                            if request.starts_with(b"GET /proxy.pac HTTP/") {
                                let response = format!("HTTP/1.0 200 OK\r\nContent-Type: application/x-ns-proxy-autoconfig\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{script}", script.len());
                                // 固定总截止时间避免慢读者让正常 Stop 无界等待。
                                let deadline = Instant::now() + Duration::from_millis(500);
                                let mut remaining = response.as_bytes();
                                while !remaining.is_empty() && !closing.load(Ordering::Acquire) {
                                    let budget = deadline.saturating_duration_since(Instant::now());
                                    if budget.is_zero() { break; }
                                    let _ = stream.set_write_timeout(Some(budget));
                                    match stream.write(remaining) { Ok(0) | Err(_) => break, Ok(n) => remaining = &remaining[n..] }
                                }
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                    Err(_) => break,
                }
            }
        }).map_err(|_| "pac_endpoint_thread")?;
        Ok(Self {
            url,
            stop,
            handle: Some(handle),
        })
    }
    pub(super) fn url(&self) -> &str {
        &self.url
    }
    pub(super) fn alive(&self) -> bool {
        self.handle.as_ref().is_some_and(|h| !h.is_finished())
    }
    pub(super) fn close(&mut self) -> Result<()> {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            handle.join().map_err(|_| "pac_endpoint_join")?;
        }
        Ok(())
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
