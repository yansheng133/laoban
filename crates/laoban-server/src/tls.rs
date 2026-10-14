//! TLS for the aggregated API port: the server certificate the API server
//! trusts (APIService `caBundle`), and client certificates required from
//! the request-header CA (see `identity`).

use std::io;
use std::net::TcpStream;
use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig, ServerConnection, StreamOwned};

use crate::identity::AuthConfig;

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Server config: our certificate and key (PEM), client certificates
/// required and verified against the request-header CA.
pub fn server_config(
    cert_pem: &[u8],
    key_pem: &[u8],
    auth: &AuthConfig,
) -> Result<Arc<ServerConfig>, String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
        .collect::<Result<_, _>>()
        .map_err(|_| "server certificate is not valid PEM".to_string())?;
    if certs.is_empty() {
        return Err("no server certificate".into());
    }
    let key = PrivateKeyDer::from_pem_slice(key_pem)
        .map_err(|_| "server key is not valid PEM".to_string())?;
    let mut roots = RootCertStore::empty();
    for ca in CertificateDer::pem_slice_iter(auth.client_ca_pem.as_bytes()) {
        let ca = ca.map_err(|_| "request-header CA is not valid PEM".to_string())?;
        roots
            .add(ca)
            .map_err(|_| "request-header CA is not a valid certificate".to_string())?;
    }
    if roots.is_empty() {
        return Err("no request-header CA certificate".into());
    }
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider())
        .build()
        .map_err(|e| format!("client verifier: {e}"))?;
    let config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS versions: {e}"))?
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)
        .map_err(|e| format!("server certificate: {e}"))?;
    Ok(Arc::new(config))
}

/// Common name of a DER certificate's subject.
pub fn common_name(der: &[u8]) -> Option<String> {
    let (_, cert) = x509_parser::parse_x509_certificate(der).ok()?;
    let cn = cert.subject().iter_common_name().next()?;
    cn.as_str().ok().map(str::to_string)
}

pub type TlsStream = StreamOwned<ServerConnection, TcpStream>;

/// Complete the handshake (bounded by the socket's timeouts) and check the
/// client certificate's name. The chain was already verified by rustls.
pub fn accept(
    config: &Arc<ServerConfig>,
    auth: &AuthConfig,
    mut sock: TcpStream,
) -> io::Result<TlsStream> {
    let mut conn = ServerConnection::new(Arc::clone(config)).map_err(io::Error::other)?;
    while conn.is_handshaking() {
        conn.complete_io(&mut sock)?;
    }
    let cn = conn
        .peer_certificates()
        .and_then(|c| c.first())
        .and_then(|c| common_name(c.as_ref()));
    if !auth.name_allowed(cn.as_deref()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "client certificate name not allowed",
        ));
    }
    Ok(StreamOwned::new(conn, sock))
}
