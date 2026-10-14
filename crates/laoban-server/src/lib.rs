//! Minimal HTTP server for the laoban backend.
//!
//! Endpoints:
//!
//! - `GET /healthz` returns `200 ok` (for liveness and readiness probes).
//! - `GET /version` returns build metadata as JSON.
//! - `POST /api/plan`, `POST /api/apply`, `GET /api/config`: the M0
//!   pipeline (`laoban-m0`). JSON in and out; errors are
//!   `{"error": {"code", "message"}}` with a Traditional Chinese message.
//!
//! The server is deliberately std-only: one thread per connection, strict
//! limits on request size, socket timeouts and concurrent connections, and
//! `Connection: close` on every response.
//!
//! Timeouts: the read and write timeouts apply to each socket operation,
//! not to the whole request. Reading the request (head and body) must make
//! progress every [`IO_TIMEOUT`]; while `/api/plan` or `/api/apply` waits
//! for the AI and GitHub (tens of seconds, bounded by those clients' own
//! timeouts) the socket is idle and no timeout fires. The response is then
//! written with [`API_WRITE_TIMEOUT`].
//!
//! Every request produces one log line on stderr: method, path (without
//! query string), status, duration, and the plan ID when there is one.
//! Bodies, prompts, generated code, tokens and keys are never logged.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use laoban_m0::{
    ApplyRequest, M0Error, M0Service, PlanRequestBody, RestoreRequest, SaveFilesRequest,
    SaveLayoutRequest,
};
use serde_json::json;

pub mod identity;
pub mod kube;
pub mod tls;

use identity::{AuthConfig, Caller};

/// Package name, baked in at compile time.
pub const NAME: &str = env!("CARGO_PKG_NAME");
/// Package version, baked in at compile time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Git commit the binary was built from, taken from the `GIT_SHA`
/// environment variable at compile time (`"unknown"` when unset or empty).
pub fn git_sha() -> &'static str {
    match option_env!("GIT_SHA") {
        Some(sha) if !sha.trim().is_empty() => sha,
        _ => "unknown",
    }
}

/// Body of `GET /version`.
pub fn version_json() -> String {
    serde_json::json!({
        "name": NAME,
        "version": VERSION,
        "git_sha": git_sha(),
    })
    .to_string()
}

/// Largest request head (request line plus headers) accepted, in bytes.
const MAX_HEAD_BYTES: usize = 8 * 1024;
/// Largest request body accepted on `/api/*`, in bytes.
/// Hand-edited code (up to 192 KiB of files, JSON-escaped) must fit.
pub const MAX_BODY_BYTES: usize = 512 * 1024;
/// At most this many request headers are kept.
const MAX_HEADERS: usize = 100;
/// Upper bound on connections handled at the same time.
const MAX_CONNECTIONS: usize = 64;
/// Read and write timeout per socket operation while receiving a request
/// and for the probe endpoints.
pub const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// Write timeout for `/api/*` responses (bodies are larger; the client may
/// be a browser behind the Rancher proxy).
pub const API_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `run` waits for in-flight connections after shutdown. Below
/// Kubernetes' default 30 s termination grace period.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(25);

/// What `/api/*` serves.
pub enum ApiState {
    Ready(Arc<M0Service>),
    /// Required configuration is missing or invalid. `/healthz` and
    /// `/version` still work; `/api/*` answers 500 `internal` with this
    /// message, which names variables but never their values.
    Unconfigured(String),
}

/// Where the caller of `/api/*` (and the aggregated API paths) comes from.
#[derive(Clone)]
enum Access {
    /// Only `/healthz` and `/version` (the plain port in production).
    ProbesOnly,
    /// The aggregated API port: mutual TLS with the API server, user from
    /// the request headers it sets.
    Verified(Arc<AuthConfig>),
    /// Tests only: `/api/*` on the plain port, user from `X-Remote-User`
    /// without any verification.
    InsecureHeaders,
}

/// The aggregated API port.
struct TlsPort {
    listener: TcpListener,
    local_addr: SocketAddr,
    config: Arc<rustls::ServerConfig>,
    auth: Arc<AuthConfig>,
}

/// A bound, not yet running server.
pub struct Server {
    listener: TcpListener,
    local_addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    api: Arc<ApiState>,
    plain_access: Access,
    tls: Option<TlsPort>,
}

/// Cloneable handle that stops a running [`Server`].
#[derive(Clone)]
pub struct ShutdownHandle {
    flag: Arc<AtomicBool>,
    wake_addr: SocketAddr,
    wake_tls: Option<SocketAddr>,
}

impl ShutdownHandle {
    /// Ask the server to stop accepting connections. `Server::run` returns
    /// once in-flight connections finish (bounded by a drain timeout).
    pub fn shutdown(&self) {
        self.flag.store(true, Ordering::SeqCst);
        // `accept` blocks, so poke the listener to make the loop notice the
        // flag. A failure here only means the listener is already gone.
        let _ = TcpStream::connect_timeout(&self.wake_addr, Duration::from_secs(1));
        if let Some(addr) = self.wake_tls {
            let _ = TcpStream::connect_timeout(&addr, Duration::from_secs(1));
        }
    }
}

impl Server {
    /// Bind to `addr` (for example `0.0.0.0:8080`, or `127.0.0.1:0` for an
    /// ephemeral port) without the M0 API configured.
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        Self::bind_with_api(
            addr,
            ApiState::Unconfigured("後端 API 尚未設定。".to_string()),
        )
    }

    /// Bind and serve `/api/*` from `api`.
    pub fn bind_with_api<A: ToSocketAddrs>(addr: A, api: ApiState) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let local_addr = listener.local_addr()?;
        Ok(Self {
            listener,
            local_addr,
            shutdown: Arc::new(AtomicBool::new(false)),
            api: Arc::new(api),
            plain_access: Access::ProbesOnly,
            tls: None,
        })
    }

    /// Also serve the API as a Kubernetes aggregated API over TLS on
    /// `addr`, with the server certificate and key (PEM) and the API
    /// server's request-header settings.
    pub fn with_tls<A: ToSocketAddrs>(
        mut self,
        addr: A,
        cert_pem: &[u8],
        key_pem: &[u8],
        auth: AuthConfig,
    ) -> io::Result<Self> {
        let config = tls::server_config(cert_pem, key_pem, &auth)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let listener = TcpListener::bind(addr)?;
        let local_addr = listener.local_addr()?;
        self.tls = Some(TlsPort {
            listener,
            local_addr,
            config,
            auth: Arc::new(auth),
        });
        Ok(self)
    }

    /// Tests only: serve `/api/*` on the plain port and take the user from
    /// an unverified `X-Remote-User` header. Never used by the binary.
    #[doc(hidden)]
    pub fn insecure_plain_api_for_tests(mut self) -> Self {
        self.plain_access = Access::InsecureHeaders;
        self
    }

    /// The TLS port's address, if any.
    pub fn tls_addr(&self) -> Option<SocketAddr> {
        self.tls.as_ref().map(|t| t.local_addr)
    }

    /// The address actually bound (useful with port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Handle used to stop the server from another thread or a signal handler.
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        let wake_addr = loopback(self.local_addr);
        let wake_tls = self.tls.as_ref().map(|t| loopback(t.local_addr));
        ShutdownHandle {
            flag: Arc::clone(&self.shutdown),
            wake_addr,
            wake_tls,
        }
    }

    /// Serve until [`ShutdownHandle::shutdown`] is called.
    pub fn run(self) -> io::Result<()> {
        let active = Arc::new(AtomicUsize::new(0));
        let tls_thread = self.tls.map(|port| {
            let shutdown = Arc::clone(&self.shutdown);
            let active = Arc::clone(&active);
            let api = Arc::clone(&self.api);
            thread::spawn(move || {
                let access = Access::Verified(Arc::clone(&port.auth));
                let config = Arc::clone(&port.config);
                let auth = Arc::clone(&port.auth);
                accept_loop(&port.listener, &shutdown, &active, move |stream| {
                    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
                    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                    // Not the API server (or a broken handshake): no
                    // response, nothing to log per connection.
                    if let Ok(tls) = tls::accept(&config, &auth, stream) {
                        let _ = handle_connection(tls, &api, &access);
                    }
                });
            })
        });
        let api = Arc::clone(&self.api);
        let access = self.plain_access.clone();
        accept_loop(&self.listener, &self.shutdown, &active, move |stream| {
            let _ = handle_connection(stream, &api, &access);
        });
        if let Some(t) = tls_thread {
            let _ = t.join();
        }

        let deadline = Instant::now() + DRAIN_TIMEOUT;
        while active.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
}

fn loopback(mut addr: SocketAddr) -> SocketAddr {
    // Connecting to the unspecified address is not portable; use loopback.
    if addr.ip().is_unspecified() {
        addr.set_ip(match addr {
            SocketAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
            SocketAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
        });
    }
    addr
}

/// Accept connections until shutdown, one thread each, at most
/// [`MAX_CONNECTIONS`] at a time across both ports.
fn accept_loop<F>(
    listener: &TcpListener,
    shutdown: &Arc<AtomicBool>,
    active: &Arc<AtomicUsize>,
    handle: F,
) where
    F: Fn(TcpStream) + Send + Sync + Clone + 'static,
{
    for conn in listener.incoming() {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        let stream = match conn {
            Ok(stream) => stream,
            // Transient accept errors (for example the peer reset before
            // accept, or a file descriptor limit) must not stop the server.
            Err(_) => continue,
        };
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::SeqCst);
            // Dropped: on the TLS port a plain-text answer would be noise.
            continue;
        }
        let worker_active = Arc::clone(active);
        let handle = handle.clone();
        let spawned = thread::Builder::new()
            .name("laoban-conn".into())
            .spawn(move || {
                handle(stream);
                worker_active.fetch_sub(1, Ordering::SeqCst);
            });
        if let Err(e) = spawned {
            // The closure never ran (the stream was dropped with it), so
            // release the slot here.
            active.fetch_sub(1, Ordering::SeqCst);
            eprintln!("laoban-server: failed to spawn connection thread: {e}");
        }
    }
}

/// A connection the handler reads the request from and writes the
/// response to: plain TCP or TLS over TCP.
trait Conn: Read + Write {
    fn tcp(&self) -> &TcpStream;
    /// Close gracefully (see [`linger_close`]).
    fn finish(self);
}

impl Conn for TcpStream {
    fn tcp(&self) -> &TcpStream {
        self
    }
    fn finish(self) {
        linger_close(self);
    }
}

impl Conn for tls::TlsStream {
    fn tcp(&self) -> &TcpStream {
        &self.sock
    }
    fn finish(mut self) {
        self.conn.send_close_notify();
        let _ = self.conn.complete_io(&mut self.sock);
        linger_close(self.sock);
    }
}

struct Response {
    status: u16,
    content_type: &'static str,
    body: String,
    allow: Option<&'static str>,
    /// For the log line only.
    plan_id: Option<String>,
    error_code: Option<&'static str>,
    /// For the log line only: why it failed (see `M0Error::log_detail`).
    error_detail: Option<String>,
}

impl Response {
    fn text(status: u16, body: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: body.to_owned(),
            allow: None,
            plan_id: None,
            error_code: None,
            error_detail: None,
        }
    }

    fn json(status: u16, body: String) -> Self {
        Self {
            status,
            content_type: "application/json",
            body,
            allow: None,
            plan_id: None,
            error_code: None,
            error_detail: None,
        }
    }

    /// The design's error format.
    fn api_error(status: u16, code: &'static str, message: &str) -> Self {
        Self {
            error_code: Some(code),
            error_detail: None,
            ..Self::json(
                status,
                json!({ "error": { "code": code, "message": message } }).to_string(),
            )
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        402 => "Payment Required",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        413 => "Content Too Large",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

fn write_response<W: Write>(stream: &mut W, resp: &Response, head_only: bool) -> io::Result<()> {
    let mut out = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n",
        resp.status,
        reason(resp.status),
        resp.content_type,
        resp.body.len(),
    );
    if let Some(allow) = resp.allow {
        out.push_str("Allow: ");
        out.push_str(allow);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    if !head_only {
        out.push_str(&resp.body);
    }
    stream.write_all(out.as_bytes())?;
    stream.flush()
}

enum Head {
    /// Peer closed without sending anything (TCP probes do this).
    Empty,
    /// The head, and any body bytes read past it.
    Complete(Vec<u8>, Vec<u8>),
}

fn io_status(e: &io::Error) -> u16 {
    if matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ) {
        408
    } else {
        400
    }
}

fn read_head<R: Read>(stream: &mut R) -> Result<Head, u16> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = match stream.read(&mut chunk) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_status(&e)),
        };
        if n == 0 {
            return if buf.is_empty() {
                Ok(Head::Empty)
            } else {
                Err(400)
            };
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = find_head_end(&buf) {
            let rest = buf.split_off(end);
            return Ok(Head::Complete(buf, rest));
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(431);
        }
    }
}

/// Index just past the blank line ending the head, accepting bare LF too.
fn find_head_end(buf: &[u8]) -> Option<usize> {
    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
        return Some(i + 4);
    }
    buf.windows(2).position(|w| w == b"\n\n").map(|i| i + 2)
}

/// A parsed request head.
struct RequestHead {
    method: String,
    /// Path without the query string.
    path: String,
    content_length: Option<usize>,
    chunked: bool,
    json_content_type: bool,
    /// All headers as (lower-case name, trimmed value), at most
    /// [`MAX_HEADERS`].
    headers: Vec<(String, String)>,
}

fn parse_head(head: &[u8]) -> Result<RequestHead, u16> {
    let text = std::str::from_utf8(head).map_err(|_| 400u16)?;
    let mut lines = text.lines();
    let request_line = lines.next().unwrap_or("").trim_end_matches('\r');
    let mut parts = request_line.split(' ');
    let (method, target, version) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(m), Some(t), Some(v), None) if !m.is_empty() && !t.is_empty() => (m, t, v),
        _ => return Err(400),
    };
    if !version.starts_with("HTTP/1.") {
        return Err(400);
    }
    let path = target.split('?').next().unwrap_or(target);

    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    let mut json_content_type = false;
    let mut headers = Vec::new();
    for line in lines {
        let line = line.trim_end_matches('\r');
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if headers.len() >= MAX_HEADERS {
            return Err(431);
        }
        headers.push((name.trim().to_ascii_lowercase(), value.to_string()));
        if name.eq_ignore_ascii_case("content-length") {
            let n: usize = value
                .parse()
                .ok()
                .filter(|_| value.bytes().all(|b| b.is_ascii_digit()))
                .ok_or(400u16)?;
            // Conflicting duplicates are a request smuggling signal.
            if content_length.is_some_and(|prev| prev != n) {
                return Err(400);
            }
            content_length = Some(n);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            // Only plain chunked; anything else (gzip, a list) is refused.
            if !value.eq_ignore_ascii_case("chunked") || chunked {
                return Err(400);
            }
            chunked = true;
        } else if name.eq_ignore_ascii_case("content-type") {
            let mime = value.split(';').next().unwrap_or("").trim();
            json_content_type = mime.eq_ignore_ascii_case("application/json");
        }
    }
    // Both framings at once is a request smuggling signal.
    if chunked && content_length.is_some() {
        return Err(400);
    }
    Ok(RequestHead {
        method: method.to_string(),
        path: path.to_string(),
        content_length,
        chunked,
        json_content_type,
        headers,
    })
}

/// Read the rest of a body whose first bytes may already be in `body`.
fn read_body<R: Read>(stream: &mut R, mut body: Vec<u8>, len: usize) -> Result<Vec<u8>, u16> {
    let mut chunk = [0u8; 4096];
    while body.len() < len {
        match stream.read(&mut chunk) {
            Ok(0) => return Err(400),
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_status(&e)),
        }
    }
    body.truncate(len);
    Ok(body)
}

/// Read a chunked body (RFC 9112 section 7.1) whose first bytes may already
/// be in `rest`, at most `max` bytes of content. The Kubernetes API server
/// forwards some requests to an aggregated API this way even when the
/// client sent Content-Length. Extensions are ignored, trailers discarded.
fn read_chunked<R: Read>(stream: &mut R, rest: Vec<u8>, max: usize) -> Result<Vec<u8>, u16> {
    struct Buf<'a, R> {
        stream: &'a mut R,
        data: Vec<u8>,
        pos: usize,
    }
    impl<R: Read> Buf<'_, R> {
        fn fill(&mut self) -> Result<(), u16> {
            let mut chunk = [0u8; 4096];
            loop {
                match self.stream.read(&mut chunk) {
                    Ok(0) => return Err(400),
                    Ok(n) => {
                        self.data.drain(..self.pos);
                        self.pos = 0;
                        self.data.extend_from_slice(&chunk[..n]);
                        return Ok(());
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(io_status(&e)),
                }
            }
        }
        /// One line without its CRLF (bare LF accepted), at most 4 KiB.
        fn line(&mut self) -> Result<Vec<u8>, u16> {
            loop {
                if let Some(i) = self.data[self.pos..].iter().position(|&b| b == b'\n') {
                    let mut line = self.data[self.pos..self.pos + i].to_vec();
                    self.pos += i + 1;
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    return Ok(line);
                }
                if self.data.len() - self.pos > 4096 {
                    return Err(400);
                }
                self.fill()?;
            }
        }
        fn take(&mut self, n: usize, out: &mut Vec<u8>) -> Result<(), u16> {
            while self.data.len() - self.pos < n {
                self.fill()?;
            }
            out.extend_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(())
        }
    }
    let mut buf = Buf {
        stream,
        data: rest,
        pos: 0,
    };
    let mut body = Vec::new();
    loop {
        let line = buf.line()?;
        let size_text = std::str::from_utf8(&line).map_err(|_| 400u16)?;
        let size_text = size_text.split(';').next().unwrap_or("").trim();
        if size_text.is_empty()
            || size_text.len() > 8
            || !size_text.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(400);
        }
        let size = usize::from_str_radix(size_text, 16).map_err(|_| 400u16)?;
        if size == 0 {
            // Trailers until the empty line.
            while !buf.line()?.is_empty() {}
            return Ok(body);
        }
        if body.len() + size > max {
            return Err(413);
        }
        buf.take(size, &mut body)?;
        if !buf.line()?.is_empty() {
            return Err(400);
        }
    }
}

fn route_basic(head: &RequestHead) -> (Response, bool) {
    let head_only = head.method == "HEAD";
    let get_like = head.method == "GET" || head_only;
    let resp = match head.path.as_str() {
        "/healthz" if get_like => Response::text(200, "ok"),
        "/version" if get_like => Response::json(200, version_json()),
        "/healthz" | "/version" => Response {
            allow: Some("GET, HEAD"),
            ..Response::text(405, "method not allowed")
        },
        _ => Response::text(404, "not found"),
    };
    (resp, head_only)
}

#[cfg(test)]
fn route(head: &[u8]) -> (Response, bool) {
    match parse_head(head) {
        Ok(h) => route_basic(&h),
        Err(status) => (Response::text(status, reason(status)), false),
    }
}

fn m0_error(e: &M0Error) -> Response {
    let mut resp = Response::api_error(e.status(), e.code(), &e.user_message());
    resp.error_detail = e.log_detail();
    resp
}

fn bad_request(message: &str) -> Response {
    Response::api_error(400, "bad_request", message)
}

fn valid_plan_id(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() == 17
        && id.starts_with("p-")
        && b[2..10].iter().all(u8::is_ascii_digit)
        && b[10] == b'-'
        && b[11..]
            .iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
}

/// Handle `/api/*`. `body` is empty for requests without one.
/// The `/api/*` endpoints.
enum Route<'a> {
    Config,
    Apps,
    App(&'a str),
    Versions(&'a str),
    Version(&'a str, &'a str),
    Restore(&'a str),
    Lock(&'a str),
    RenewLock(&'a str),
    Unlock(&'a str),
    Files(&'a str),
    Layout(&'a str),
    Plan,
    Apply,
    /// `POST .../plans/<id>/apply` (aggregated API path).
    ApplyPlan(&'a str),
}

impl<'a> Route<'a> {
    fn parse(path: &'a str) -> Option<Self> {
        match path {
            "/api/config" => return Some(Route::Config),
            "/api/apps" => return Some(Route::Apps),
            "/api/plan" => return Some(Route::Plan),
            "/api/apply" => return Some(Route::Apply),
            _ => {}
        }
        if let Some(rest) = path.strip_prefix("/api/plans/") {
            return match rest.split('/').collect::<Vec<_>>().as_slice() {
                [id, "apply"] if !id.is_empty() => Some(Route::ApplyPlan(id)),
                _ => None,
            };
        }
        let rest = path.strip_prefix("/api/apps/")?;
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.iter().any(|p| p.is_empty()) {
            return None;
        }
        match parts.as_slice() {
            [name] => Some(Route::App(name)),
            [name, "versions"] => Some(Route::Versions(name)),
            [name, "versions", commit] => Some(Route::Version(name, commit)),
            [name, "restore"] => Some(Route::Restore(name)),
            [name, "lock"] => Some(Route::Lock(name)),
            [name, "lock", "renew"] => Some(Route::RenewLock(name)),
            [name, "unlock"] => Some(Route::Unlock(name)),
            [name, "files"] => Some(Route::Files(name)),
            [name, "layout"] => Some(Route::Layout(name)),
            _ => None,
        }
    }

    fn method(&self, request_method: &str) -> &'static str {
        match self {
            // Read for the editor, POST to save.
            Route::Files(_) if request_method == "POST" => "POST",
            Route::Config
            | Route::Apps
            | Route::App(_)
            | Route::Versions(_)
            | Route::Version(..)
            | Route::Files(_) => "GET",
            _ => "POST",
        }
    }
}

/// Handle `/api/*` and the aggregated API paths (`/apis/...`). `body` is
/// empty for requests without one; `caller` is the verified user.
fn route_api(api: &ApiState, head: &RequestHead, body: &[u8], caller: Option<&Caller>) -> Response {
    let gv = format!("/apis/{}/{}", identity::GROUP, identity::VERSION);
    if head.method == "GET" && head.path == gv {
        return Response::json(200, identity::discovery_json());
    }
    if head.method == "GET" && head.path == format!("/apis/{}", identity::GROUP) {
        return Response::json(200, identity::group_json());
    }
    let internal;
    let path = if head.path.starts_with("/apis/") {
        let ns = match api {
            ApiState::Ready(s) => s.target_namespace().to_string(),
            ApiState::Unconfigured(message) => {
                return Response::api_error(500, "internal", message)
            }
        };
        match identity::internal_path(&head.path, &ns) {
            Some(p) => {
                internal = p;
                internal.as_str()
            }
            None => return Response::api_error(404, "bad_request", "找不到這個 API。"),
        }
    } else {
        head.path.as_str()
    };
    let Some(route) = Route::parse(path) else {
        return Response::api_error(404, "bad_request", "找不到這個 API。");
    };
    let allowed = route.method(&head.method);
    if head.method != allowed {
        let allow = if matches!(route, Route::Files(_)) {
            "GET, POST"
        } else {
            allowed
        };
        return Response {
            allow: Some(allow),
            ..Response::api_error(405, "bad_request", "不支援這個 HTTP 方法。")
        };
    }
    let service = match api {
        ApiState::Ready(s) => s,
        ApiState::Unconfigured(message) => return Response::api_error(500, "internal", message),
    };
    let reply = |result: Result<serde_json::Value, M0Error>| match result {
        Ok(v) => Response::json(200, v.to_string()),
        Err(e) => m0_error(&e),
    };
    if allowed == "GET" {
        return reply(match route {
            Route::Config => Ok(service.config_json()),
            Route::Apps => service.list_apps(),
            Route::App(name) => service.get_app(name),
            Route::Versions(name) => service.versions(name),
            Route::Version(name, commit) => service.version(name, commit),
            Route::Files(name) => service.files(name),
            _ => return Response::api_error(404, "bad_request", "找不到這個 API。"),
        });
    }
    let lock = caller.map(|c| c.user.as_str());
    // Lock calls carry no body; the user is the verified caller.
    match route {
        Route::Lock(name) => return reply(service.lock(name, lock)),
        Route::RenewLock(name) => return reply(service.renew_lock(name, lock)),
        Route::Unlock(name) => return reply(service.unlock(name, lock)),
        Route::ApplyPlan(id) => {
            let mut resp = reply(service.apply(
                &ApplyRequest {
                    plan_id: id.to_string(),
                },
                lock,
            ));
            resp.plan_id = Some(id.to_string()).filter(|id| valid_plan_id(id));
            return resp;
        }
        _ => {}
    }
    // Requiring a JSON content type also keeps plain cross-site form
    // posts (which cannot set it without a CORS preflight) out.
    if !head.json_content_type {
        return bad_request("請以 JSON 送出（Content-Type: application/json）。");
    }
    match route {
        Route::Plan => {
            let Ok(req) = serde_json::from_slice::<PlanRequestBody>(body) else {
                return bad_request(
                    "請求格式不正確：需要 {\"prompt\": \"…\"}，可加 \"appName\" 或 \"baseApp\"。",
                );
            };
            match service.plan(&req, lock) {
                Ok(v) => Response {
                    plan_id: v["planId"].as_str().map(str::to_string),
                    ..Response::json(200, v.to_string())
                },
                Err(e) => m0_error(&e),
            }
        }
        Route::Apply => {
            let Ok(req) = serde_json::from_slice::<ApplyRequest>(body) else {
                return bad_request("請求格式不正確：需要 {\"planId\": \"…\"}。");
            };
            let mut resp = reply(service.apply(&req, lock));
            // Only log IDs that look like ours; the field is client input.
            resp.plan_id = Some(req.plan_id).filter(|id| valid_plan_id(id));
            resp
        }
        Route::Restore(name) => {
            let Ok(req) = serde_json::from_slice::<RestoreRequest>(body) else {
                return bad_request("請求格式不正確：需要 {\"commit\": \"…\"}。");
            };
            reply(service.restore(name, &req, lock))
        }
        Route::Files(name) => {
            let Ok(req) = serde_json::from_slice::<SaveFilesRequest>(body) else {
                return bad_request(
                    "請求格式不正確：需要 {\"baseCommit\": \"…\", \"files\": {…}}。",
                );
            };
            reply(service.save_files(name, &req, lock))
        }
        Route::Layout(name) => {
            let Ok(req) = serde_json::from_slice::<SaveLayoutRequest>(body) else {
                return bad_request(
                    "請求格式不正確：需要 {\"baseCommit\": \"…\", \"positions\": {…}}。",
                );
            };
            reply(service.save_layout(name, &req, lock))
        }
        _ => Response::api_error(404, "bad_request", "找不到這個 API。"),
    }
}

/// Keep log lines on one line and short; paths are client-controlled.
fn log_safe(s: &str) -> String {
    s.chars()
        .take(200)
        .map(|c| if c.is_control() || c == ' ' { '?' } else { c })
        .collect()
}

/// One line, bounded, quoted; spaces kept so the message stays readable.
fn log_detail_safe(s: &str) -> String {
    let inner: String = s
        .chars()
        .take(300)
        .map(|c| if c.is_control() || c == '"' { '\'' } else { c })
        .collect();
    format!("\"{inner}\"")
}

fn log_line(method: &str, path: &str, resp: &Response, started: Instant, user: Option<&str>) {
    let mut line = format!(
        "{NAME}: {} {} {} {}ms",
        log_safe(method),
        log_safe(path),
        resp.status,
        started.elapsed().as_millis()
    );
    if let Some(u) = user {
        line.push_str(" user=");
        line.push_str(&log_safe(u));
    }
    if let Some(id) = &resp.plan_id {
        line.push_str(" planId=");
        line.push_str(id);
    }
    if let Some(code) = resp.error_code {
        line.push_str(" error=");
        line.push_str(code);
    }
    if let Some(detail) = &resp.error_detail {
        line.push_str(" detail=");
        line.push_str(&log_detail_safe(detail));
    }
    eprintln!("{line}");
}

fn handle_connection<C: Conn>(mut stream: C, api: &ApiState, access: &Access) -> io::Result<()> {
    let started = Instant::now();
    stream.tcp().set_read_timeout(Some(IO_TIMEOUT))?;
    stream.tcp().set_write_timeout(Some(IO_TIMEOUT))?;
    let (head_bytes, rest) = match read_head(&mut stream) {
        Ok(Head::Empty) => return Ok(()),
        Ok(Head::Complete(head, rest)) => (head, rest),
        Err(status) => {
            let resp = Response::text(status, reason(status));
            log_line("-", "-", &resp, started, None);
            write_response(&mut stream, &resp, false)?;
            stream.finish();
            return Ok(());
        }
    };
    let head = match parse_head(&head_bytes) {
        Ok(h) => h,
        Err(status) => {
            let resp = Response::text(status, reason(status));
            log_line("-", "-", &resp, started, None);
            write_response(&mut stream, &resp, false)?;
            stream.finish();
            return Ok(());
        }
    };

    let is_api =
        head.path == "/api" || head.path.starts_with("/api/") || head.path.starts_with("/apis/");
    let caller = match access {
        Access::Verified(auth) => auth.caller(&head.headers),
        Access::InsecureHeaders => head
            .headers
            .iter()
            .find(|(n, v)| n == "x-remote-user" && !v.is_empty())
            .map(|(_, v)| Caller {
                user: v.clone(),
                groups: Vec::new(),
            }),
        Access::ProbesOnly => None,
    };
    let (resp, head_only) = if is_api && matches!(access, Access::ProbesOnly) {
        (
            Response::api_error(
                404,
                "bad_request",
                "這個連接埠只提供健康檢查；API 經 Kubernetes API server 提供。",
            ),
            false,
        )
    } else if is_api {
        let too_large =
            || Response::api_error(413, "bad_request", "請求內容太大（上限 512 KiB）。");
        let resp = if head.chunked {
            match read_chunked(&mut stream, rest, MAX_BODY_BYTES) {
                Ok(body) => route_api(api, &head, &body, caller.as_ref()),
                Err(413) => too_large(),
                Err(status) => Response::text(status, reason(status)),
            }
        } else {
            match head.content_length.unwrap_or(0) {
                n if n > MAX_BODY_BYTES => too_large(),
                n => match read_body(&mut stream, rest, n) {
                    Ok(body) => route_api(api, &head, &body, caller.as_ref()),
                    Err(status) => Response::text(status, reason(status)),
                },
            }
        };
        // Generating a plan or code can take a long time; the response
        // itself gets a generous write timeout.
        stream.tcp().set_write_timeout(Some(API_WRITE_TIMEOUT))?;
        (resp, false)
    } else {
        route_basic(&head)
    };
    log_line(
        &head.method,
        &head.path,
        &resp,
        started,
        caller.as_ref().map(|c| c.user.as_str()),
    );
    write_response(&mut stream, &resp, head_only)?;
    stream.finish();
    Ok(())
}

/// Close gracefully: if the client sent more than we read (a body, or an
/// oversized head), closing straight away makes the kernel send RST, which
/// can destroy the response before the client reads it. Half-close, then
/// discard what is left for a short, bounded time.
fn linger_close(mut stream: TcpStream) {
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut sink = [0u8; 4096];
    let mut drained = 0usize;
    let deadline = Instant::now() + Duration::from_secs(1);
    while drained < MAX_BODY_BYTES + 64 * 1024 && Instant::now() < deadline {
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(n) => drained += n,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_of(req: &str) -> u16 {
        route(req.as_bytes()).0.status
    }

    #[test]
    fn routes() {
        assert_eq!(status_of("GET /healthz HTTP/1.1\r\n\r\n"), 200);
        assert_eq!(status_of("GET /healthz?x=1 HTTP/1.0\r\n\r\n"), 200);
        assert_eq!(status_of("HEAD /version HTTP/1.1\r\n\r\n"), 200);
        assert_eq!(status_of("POST /healthz HTTP/1.1\r\n\r\n"), 405);
        assert_eq!(status_of("GET / HTTP/1.1\r\n\r\n"), 404);
        assert_eq!(status_of("GET /healthz\r\n\r\n"), 400);
        assert_eq!(status_of("GET /healthz SPDY/3\r\n\r\n"), 400);
        assert_eq!(status_of("GET  /healthz HTTP/1.1\r\n\r\n"), 400);
    }

    #[test]
    fn head_end_detection() {
        assert_eq!(find_head_end(b"GET / HTTP/1.1\r\n\r\nrest"), Some(18));
        assert_eq!(find_head_end(b"GET / HTTP/1.1\n\n"), Some(16));
        assert_eq!(find_head_end(b"GET / HTTP/1.1\r\n"), None);
    }

    #[test]
    fn version_json_has_fields() {
        let v: serde_json::Value = serde_json::from_str(&version_json()).unwrap();
        assert_eq!(v["name"], NAME);
        assert_eq!(v["version"], VERSION);
        assert!(v["git_sha"].as_str().is_some_and(|s| !s.is_empty()));
    }

    #[test]
    fn head_parsing() {
        let h = parse_head(
            b"POST /api/plan?x=1 HTTP/1.1\r\nContent-Length: 12\r\ncontent-type: application/json; charset=utf-8\r\n\r\n",
        )
        .ok()
        .unwrap();
        assert_eq!(h.path, "/api/plan");
        assert_eq!(h.content_length, Some(12));
        assert!(h.json_content_type && !h.chunked);
        for bad in [
            "POST / HTTP/1.1\r\nContent-Length: -1\r\n\r\n",
            "POST / HTTP/1.1\r\nContent-Length: +5\r\n\r\n",
            "POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n",
        ] {
            assert_eq!(parse_head(bad.as_bytes()).err(), Some(400), "{bad}");
        }
        let h = parse_head(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n")
            .ok()
            .unwrap();
        assert!(h.chunked);
        for bad in [
            "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Length: 3\r\n\r\n",
            "POST / HTTP/1.1\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
            "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n",
        ] {
            assert_eq!(parse_head(bad.as_bytes()).err(), Some(400), "{bad}");
        }
    }

    #[test]
    fn chunked_bodies() {
        let read = |rest: &[u8], more: &[u8], max: usize| {
            read_chunked(&mut std::io::Cursor::new(more.to_vec()), rest.to_vec(), max)
        };
        // Split across what was read with the head and the rest of the stream.
        assert_eq!(read(b"4\r\n{\"a\"", b":1}\r\n0\r\n\r\n", 100), Err(400));
        assert_eq!(
            read(b"4\r\n{\"a\"\r\n3;ext=1\r\n:1}\r\n0\r\n\r\n", b"", 100).unwrap(),
            b"{\"a\":1}"
        );
        assert_eq!(read(b"", b"0\r\nX-Trailer: y\r\n\r\n", 100).unwrap(), b"");
        assert_eq!(read(b"2\r\n{}", b"\r\n0\r\n\r\n", 100).unwrap(), b"{}");
        assert_eq!(read(b"", b"a\r\n0123456789\r\n0\r\n\r\n", 5), Err(413));
        assert_eq!(read(b"", b"zz\r\n", 100), Err(400));
        assert_eq!(read(b"", b"2\r\n{}", 100), Err(400), "truncated");
    }

    #[test]
    fn plan_ids_for_logging() {
        assert!(valid_plan_id("p-20261013-1a2b3c"));
        for bad in [
            "p-2026101-1a2b3c",
            "p-20261013-1A2B3C",
            "x",
            "p-20261013-1a2b3c\n",
        ] {
            assert!(!valid_plan_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn log_safety() {
        assert_eq!(log_safe("/a b\nc\u{1b}"), "/a?b?c?");
        assert_eq!(log_safe(&"x".repeat(500)).len(), 200);
        assert_eq!(
            log_detail_safe("GitHub create blob failed with HTTP 403: \"no\"\nx"),
            "\"GitHub create blob failed with HTTP 403: 'no''x\""
        );
        assert_eq!(log_detail_safe(&"y".repeat(500)).len(), 302);
    }
}
