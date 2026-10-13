use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use laoban_server::Server;

/// Send a raw HTTP request and return (status, headers, body).
fn request(addr: SocketAddr, raw: &str) -> (u16, String, String) {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).unwrap();
    let (head, body) = out.split_once("\r\n\r\n").expect("response has a head");
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status code");
    (status, head.to_owned(), body.to_owned())
}

fn get(addr: SocketAddr, path: &str) -> (u16, String, String) {
    request(
        addr,
        &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nUser-Agent: test\r\n\r\n"),
    )
}

#[test]
fn healthz_and_version_on_ephemeral_port() {
    let server = Server::bind("127.0.0.1:0").unwrap();
    let addr = server.local_addr();
    let handle = server.shutdown_handle();
    let runner = thread::spawn(move || server.run());

    let (status, head, body) = get(addr, "/healthz");
    assert_eq!(status, 200);
    assert_eq!(body, "ok");
    assert!(head.contains("Content-Type: text/plain"));

    let (status, head, body) = get(addr, "/version");
    assert_eq!(status, 200);
    assert!(head.contains("Content-Type: application/json"));
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["name"], "laoban-server");
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert!(v["git_sha"].is_string());

    let (status, _, body) = request(addr, "HEAD /healthz HTTP/1.1\r\n\r\n");
    assert_eq!(status, 200);
    assert!(body.is_empty());

    assert_eq!(get(addr, "/nope").0, 404);
    let (status, head, _) = request(addr, "POST /healthz HTTP/1.1\r\nContent-Length: 0\r\n\r\n");
    assert_eq!(status, 405);
    assert!(head.contains("Allow: GET, HEAD"));
    assert_eq!(request(addr, "garbage\r\n\r\n").0, 400);

    let huge = format!(
        "GET /healthz HTTP/1.1\r\nX-Pad: {}\r\n\r\n",
        "a".repeat(16 * 1024)
    );
    assert_eq!(request(addr, &huge).0, 431);

    // A bare TCP connect-and-close (TCP socket probes) must not disturb the server.
    drop(TcpStream::connect(addr).unwrap());
    assert_eq!(get(addr, "/healthz").0, 200);

    handle.shutdown();
    runner.join().unwrap().unwrap();
}

#[test]
fn binary_serves_and_exits_cleanly_on_sigterm() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_laoban-server"))
        .env("LAOBAN_LISTEN_ADDR", "127.0.0.1:0")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // The first stderr line ends with "listening on <addr>".
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    let addr: SocketAddr = line
        .trim()
        .rsplit(' ')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("unexpected startup line: {line:?}"));

    let (status, _, body) = get(addr, "/healthz");
    assert_eq!((status, body.as_str()), (200, "ok"));

    // SAFETY: plain kill(2) on our own child's pid.
    let rc = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(rc, 0);

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("server did not exit within 10s of SIGTERM");
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "exit status: {status}");
}

#[test]
fn binary_version_flag() {
    let out = Command::new(env!("CARGO_BIN_EXE_laoban-server"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with(concat!("laoban-server ", env!("CARGO_PKG_VERSION"))));
}
