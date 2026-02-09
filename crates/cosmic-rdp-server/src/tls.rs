use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use rcgen::{CertificateParams, KeyPair};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::{rustls, TlsAcceptor};

/// Generate a self-signed TLS certificate and return a `TlsAcceptor`.
pub fn generate_self_signed() -> Result<TlsAcceptor> {
    tracing::info!("Generating self-signed TLS certificate");

    let key_pair = KeyPair::generate().context("failed to generate key pair")?;
    let mut params = CertificateParams::new(vec!["localhost".to_string()])
        .context("failed to create certificate params")?;
    params.distinguished_name.push(
        rcgen::DnType::CommonName,
        rcgen::DnValue::Utf8String("cosmic-rdp-server".to_string()),
    );

    let cert = params
        .self_signed(&key_pair)
        .context("failed to generate self-signed certificate")?;

    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));

    make_acceptor(&[cert_der], key_der)
}

/// Load TLS certificate and key from PEM files and return a `TlsAcceptor`.
pub fn load_from_files(cert_path: &Path, key_path: &Path) -> Result<TlsAcceptor> {
    tracing::info!(?cert_path, ?key_path, "Loading TLS certificate from files");

    let tls_ctx = ironrdp_server::TlsIdentityCtx::init_from_paths(cert_path, key_path)
        .context("failed to load TLS identity")?;

    tls_ctx.make_acceptor().context("failed to create TLS acceptor")
}

fn make_acceptor(certs: &[CertificateDer<'static>], key: PrivateKeyDer<'static>) -> Result<TlsAcceptor> {
    let mut server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs.to_vec(), key)
        .context("bad certificate/key")?;

    server_config.key_log = Arc::new(rustls::KeyLogFile::new());

    Ok(TlsAcceptor::from(Arc::new(server_config)))
}
