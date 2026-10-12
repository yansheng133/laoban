//! Minimal HTTP server for the laoban backend.
//!
//! Only two endpoints exist today:
//!
//! - `GET /healthz` returns `200 ok` (for liveness and readiness probes).
//! - `GET /version` returns build metadata as JSON.
//!
//! The server is deliberately std-only: one thread per connection, strict
//! limits on request size, socket timeouts and concurrent connections, and
//! `Connection: close` on every response. That is enough for probes and
//! smoke tests without pulling in an HTTP framework. When laoban grows a
//! real API this module is expected to be replaced by a proper framework.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

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
/// Upper bound on connections handled at the same time.
const MAX_CONNECTIONS: usize = 64;
/// Read and write timeout per connection.
const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// How long `run` waits for in-flight connections after shutdown.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// A bound, not yet running server.
pub struct Server {
    listener: TcpListener,
    local_addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
}

/// Cloneable handle that stops a running [`Server`].
#[derive(Clone)]
pub struct ShutdownHandle {
    flag: Arc<AtomicBool>,
    wake_addr: SocketAddr,
}

impl ShutdownHandle {
    /// Ask the server to stop accepting connections. `Server::run` returns
    /// once in-flight connections finish (bounded by a drain timeout).
    pub fn shutdown(&self) {
        self.flag.store(true, Ordering::SeqCst);
        // `accept` blocks, so poke the listener to make the loop notice the
        // flag. A failure here only means the listener is already gone.
        let _ = TcpStream::connect_timeout(&self.wake_addr, Duration::from_secs(1));
    }
}

impl Server {
    /// Bind to `addr` (for example `0.0.0.0:8080`, or `127.0.0.1:0` for an
    /// ephemeral port).
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let local_addr = listener.local_addr()?;
        Ok(Self {
            listener,
            local_addr,
            shutdown: Arc::new(AtomicBool::new(false)),
        })
    }

    /// The address actually bound (useful with port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Handle used to stop the server from another thread or a signal handler.
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        let mut wake_addr = self.local_addr;
        // Connecting to the unspecified address is not portable; use loopback.
        if wake_addr.ip().is_unspecified() {
            wake_addr.set_ip(match wake_addr {
                SocketAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
                SocketAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
            });
        }
        ShutdownHandle {
            flag: Arc::clone(&self.shutdown),
            wake_addr,
        }
    }

    /// Serve until [`ShutdownHandle::shutdown`] is called.
    pub fn run(self) -> io::Result<()> {
        let active = Arc::new(AtomicUsize::new(0));
        for conn in self.listener.incoming() {
            if self.shutdown.load(Ordering::SeqCst) {
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
                let mut stream = stream;
                let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                let _ = write_response(&mut stream, &Response::text(503, "busy"), false);
                continue;
            }
            let worker_active = Arc::clone(&active);
            let spawned = thread::Builder::new()
                .name("laoban-conn".into())
                .spawn(move || {
                    let _ = handle_connection(stream);
                    worker_active.fetch_sub(1, Ordering::SeqCst);
                });
            if let Err(e) = spawned {
                // The closure never ran (the stream was dropped with it), so
                // release the slot here.
                active.fetch_sub(1, Ordering::SeqCst);
                eprintln!("laoban-server: failed to spawn connection thread: {e}");
            }
        }

        let deadline = Instant::now() + DRAIN_TIMEOUT;
        while active.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
}

struct Response {
    status: u16,
    content_type: &'static str,
    body: String,
    allow: Option<&'static str>,
}

impl Response {
    fn text(status: u16, body: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: body.to_owned(),
            allow: None,
        }
    }

    fn json(status: u16, body: String) -> Self {
        Self {
            status,
            content_type: "application/json",
            body,
            allow: None,
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

fn write_response(stream: &mut TcpStream, resp: &Response, head_only: bool) -> io::Result<()> {
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
    Complete(Vec<u8>),
}

fn read_head(stream: &mut TcpStream) -> Result<Head, u16> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = match stream.read(&mut chunk) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Err(408)
            }
            Err(_) => return Err(400),
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
            buf.truncate(end);
            return Ok(Head::Complete(buf));
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

fn route(head: &[u8]) -> (Response, bool) {
    let text = match std::str::from_utf8(head) {
        Ok(t) => t,
        Err(_) => return (Response::text(400, "bad request"), false),
    };
    let request_line = text.lines().next().unwrap_or("").trim_end_matches('\r');
    let mut parts = request_line.split(' ');
    let (method, target, version) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(m), Some(t), Some(v), None) if !m.is_empty() && !t.is_empty() => (m, t, v),
        _ => return (Response::text(400, "bad request"), false),
    };
    if !version.starts_with("HTTP/1.") {
        return (Response::text(400, "bad request"), false);
    }
    let path = target.split('?').next().unwrap_or(target);
    let head_only = method == "HEAD";
    let get_like = method == "GET" || head_only;

    let resp = match path {
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

fn handle_connection(mut stream: TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let (resp, head_only) = match read_head(&mut stream) {
        Ok(Head::Empty) => return Ok(()),
        Ok(Head::Complete(head)) => route(&head),
        Err(status) => (Response::text(status, reason(status)), false),
    };
    write_response(&mut stream, &resp, head_only)?;
    linger_close(stream);
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
    while drained < 64 * 1024 && Instant::now() < deadline {
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
}
