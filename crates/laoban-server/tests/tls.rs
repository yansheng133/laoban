//! The aggregated API port: only the API server's proxy client certificate
//! (signed by the request-header CA, allowed common name) gets in, and the
//! user comes from its headers. Certificates are generated per test.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use laoban_ai::MockProvider;
use laoban_m0::{GithubCommitter, GithubConfig, GithubToken, M0Service, M0Settings};
use laoban_server::identity::{AuthConfig, GROUP, VERSION};
use laoban_server::{ApiState, Server, ShutdownHandle};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

struct Ca {
    cert: rcgen::Certificate,
    issuer: Issuer<'static, KeyPair>,
}

fn ca(name: &str) -> Ca {
    let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.distinguished_name.push(DnType::CommonName, name);
    let key = KeyPair::generate().unwrap();
    let cert = p.self_signed(&key).unwrap();
    Ca {
        cert,
        issuer: Issuer::new(p, key),
    }
}

fn leaf(ca: &Ca, cn: &str, purpose: ExtendedKeyUsagePurpose) -> (rcgen::Certificate, KeyPair) {
    let mut p = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    p.distinguished_name.push(DnType::CommonName, cn);
    p.extended_key_usages = vec![purpose];
    let key = KeyPair::generate().unwrap();
    let cert = p.signed_by(&key, &ca.issuer).unwrap();
    (cert, key)
}

fn settings() -> M0Settings {
    M0Settings {
        model: "model-a-test".into(),
        token_cap: 1_000_000,
        repo_owner: "example-owner".into(),
        repo_name: "laoban-apps".into(),
        target_namespace: "laoban-dev".into(),
        cluster_id: "c-m-test0001".into(),
        ai_timeout_secs: 300,
    }
}

struct Running {
    plain: SocketAddr,
    tls: SocketAddr,
    handle: ShutdownHandle,
    thread: Option<thread::JoinHandle<std::io::Result<()>>>,
    _github: laoban_mockhttp::MockServer,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.handle.shutdown();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Pki {
    proxy_ca: Ca,
    server_ca: Ca,
}

fn start(pki: &Pki) -> Running {
    let github = laoban_mockhttp::MockServer::start(|_| {
        laoban_mockhttp::Reply::json(404, r#"{"message":"Not Found"}"#)
    });
    let mut cfg = GithubConfig::new("example-owner", "laoban-apps");
    cfg.api_base = github.url();
    let committer = GithubCommitter::new(cfg, GithubToken::new("github_pat_test_0001").unwrap());
    let service = M0Service::new(settings(), Box::new(MockProvider::new()), committer).unwrap();
    let (server_cert, server_key) = leaf(
        &pki.server_ca,
        "laoban.laoban-system.svc",
        ExtendedKeyUsagePurpose::ServerAuth,
    );
    let auth = AuthConfig {
        client_ca_pem: pki.proxy_ca.cert.pem(),
        allowed_names: vec!["front-proxy-client".into()],
        username_headers: vec!["x-remote-user".into()],
        group_headers: vec!["x-remote-group".into()],
    };
    let server = Server::bind_with_api("127.0.0.1:0", ApiState::Ready(Arc::new(service)))
        .unwrap()
        .with_tls(
            "127.0.0.1:0",
            server_cert.pem().as_bytes(),
            server_key.serialize_pem().as_bytes(),
            auth,
        )
        .unwrap();
    let plain = server.local_addr();
    let tls = server.tls_addr().unwrap();
    let handle = server.shutdown_handle();
    let thread = Some(thread::spawn(move || server.run()));
    Running {
        plain,
        tls,
        handle,
        thread,
        _github: github,
    }
}

fn client_config(pki: &Pki, client: Option<(&rcgen::Certificate, &KeyPair)>) -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots.add(pki.server_ca.cert.der().clone()).unwrap();
    let b = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots);
    Arc::new(match client {
        Some((cert, key)) => b
            .with_client_auth_cert(
                vec![CertificateDer::from(cert.der().to_vec())],
                PrivateKeyDer::try_from(key.serialize_der()).unwrap(),
            )
            .unwrap(),
        None => b.with_no_client_auth(),
    })
}

/// Send one request over TLS; `Err` when the handshake or I/O fails.
fn tls_request(
    addr: SocketAddr,
    cfg: Arc<ClientConfig>,
    req: &str,
) -> Result<(u16, String), std::io::Error> {
    let sock = TcpStream::connect(addr)?;
    sock.set_read_timeout(Some(Duration::from_secs(10)))?;
    let conn = ClientConnection::new(cfg, ServerName::try_from("localhost").unwrap()).unwrap();
    let mut tls = StreamOwned::new(conn, sock);
    tls.write_all(req.as_bytes())?;
    let mut out = String::new();
    // A close without close_notify ends the read with UnexpectedEof.
    match tls.read_to_string(&mut out) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && !out.is_empty() => {}
        Err(e) => return Err(e),
    }
    let (head, body) = out
        .split_once("\r\n\r\n")
        .ok_or_else(|| std::io::Error::other("no response"))?;
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    Ok((status, body.to_string()))
}

fn lock_req(user: Option<&str>) -> String {
    let path = format!("/apis/{GROUP}/{VERSION}/namespaces/laoban-dev/apps/todo/round");
    let user = user
        .map(|u| format!("X-Remote-User: {u}\r\nX-Remote-Group: system:authenticated\r\n"))
        .unwrap_or_default();
    format!("POST {path} HTTP/1.1\r\nHost: x\r\n{user}Content-Type: application/json\r\nContent-Length: 35\r\n\r\n{{\"opinion\":\"x\",\"deadlineMinutes\":5}}")
}

#[test]
fn proxy_certificate_and_headers_identify_the_user() {
    let pki = Pki {
        proxy_ca: ca("request-header-ca"),
        server_ca: ca("laoban-ca"),
    };
    let r = start(&pki);
    let (proxy_cert, proxy_key) = leaf(
        &pki.proxy_ca,
        "front-proxy-client",
        ExtendedKeyUsagePurpose::ClientAuth,
    );
    let good = client_config(&pki, Some((&proxy_cert, &proxy_key)));

    // Discovery works without a user (the API server's availability check).
    let (status, body) = tls_request(
        r.tls,
        Arc::clone(&good),
        &format!("GET /apis/{GROUP}/{VERSION} HTTP/1.1\r\nHost: x\r\n\r\n"),
    )
    .unwrap();
    assert_eq!(status, 200);
    assert!(body.contains("APIResourceList"));

    // With a user: the request gets past identity (the app does not exist).
    let (status, body) =
        tls_request(r.tls, Arc::clone(&good), &lock_req(Some("u-test01"))).unwrap();
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("app_not_found"));
    // Without a user header: refused.
    let (status, body) = tls_request(r.tls, Arc::clone(&good), &lock_req(None)).unwrap();
    assert_eq!(status, 401, "{body}");

    // The plain port serves the probes only, whatever the headers say.
    let mut s = TcpStream::connect(r.plain).unwrap();
    s.write_all(lock_req(Some("u-test01")).as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 404"), "{out}");
}

#[test]
fn other_clients_cannot_connect() {
    let pki = Pki {
        proxy_ca: ca("request-header-ca"),
        server_ca: ca("laoban-ca"),
    };
    let r = start(&pki);
    // No client certificate.
    assert!(tls_request(r.tls, client_config(&pki, None), &lock_req(Some("u-admin"))).is_err());
    // Signed by the request-header CA, but not an allowed name.
    let (c, k) = leaf(
        &pki.proxy_ca,
        "someone-else",
        ExtendedKeyUsagePurpose::ClientAuth,
    );
    assert!(tls_request(
        r.tls,
        client_config(&pki, Some((&c, &k))),
        &lock_req(Some("u-admin"))
    )
    .is_err());
    // Allowed name, but signed by another CA.
    let other = ca("other-ca");
    let (c, k) = leaf(
        &other,
        "front-proxy-client",
        ExtendedKeyUsagePurpose::ClientAuth,
    );
    assert!(tls_request(
        r.tls,
        client_config(&pki, Some((&c, &k))),
        &lock_req(Some("u-admin"))
    )
    .is_err());
}
