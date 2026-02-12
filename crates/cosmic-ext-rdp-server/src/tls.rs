use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use rcgen::{CertificateParams, KeyPair, SanType};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio_rustls::rustls;
use tokio_rustls::TlsAcceptor;

/// TLS context with both the acceptor and the DER-encoded server public key.
///
/// The public key is needed for NLA/CredSSP authentication (Hybrid mode).
pub struct TlsContext {
    /// The TLS acceptor for incoming connections.
    pub acceptor: TlsAcceptor,
    /// DER-encoded server public key (for `CredSSP`).
    pub public_key: Vec<u8>,
}

/// Generate a self-signed TLS certificate and return a [`TlsContext`].
///
/// The bind address IP is included in the certificate SAN so that
/// RDP clients connecting by IP see a matching certificate.
///
/// # Errors
///
/// Returns an error if key generation or certificate creation fails.
pub fn generate_self_signed(bind_ip: IpAddr) -> Result<TlsContext> {
    tracing::info!("Generating self-signed TLS certificate");

    let key_pair = KeyPair::generate().context("failed to generate key pair")?;

    let mut san_names = vec!["localhost".to_string()];
    // Include the bind IP in SAN unless it is an unspecified address.
    let ip_str = bind_ip.to_string();
    if !bind_ip.is_unspecified() && ip_str != "localhost" {
        san_names.push(ip_str);
    }

    let mut params = CertificateParams::new(san_names)
        .context("failed to create certificate params")?;

    // Also add the bind IP as an IP SAN (not just DNS SAN).
    if !bind_ip.is_unspecified() {
        params.subject_alt_names.push(SanType::IpAddress(bind_ip));
    }

    params.distinguished_name.push(
        rcgen::DnType::CommonName,
        rcgen::DnValue::Utf8String("cosmic-ext-rdp-server".to_string()),
    );

    let cert = params
        .self_signed(&key_pair)
        .context("failed to generate self-signed certificate")?;

    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));

    // Extract public key from the certificate DER
    let public_key = extract_public_key(&cert_der);

    let acceptor = make_acceptor(&[cert_der], key_der)?;
    Ok(TlsContext {
        acceptor,
        public_key,
    })
}

/// Load TLS certificate and key from PEM files and return a [`TlsContext`].
///
/// # Errors
///
/// Returns an error if the files cannot be read or the certificate is invalid.
pub fn load_from_files(cert_path: &Path, key_path: &Path) -> Result<TlsContext> {
    tracing::info!(?cert_path, ?key_path, "Loading TLS certificate from files");

    let tls_ctx = ironrdp_server::TlsIdentityCtx::init_from_paths(cert_path, key_path)
        .context("failed to load TLS identity")?;

    let acceptor = tls_ctx
        .make_acceptor()
        .context("failed to create TLS acceptor")?;

    // Use the public key already extracted by TlsIdentityCtx (same logic as our extract_public_key)
    let public_key = tls_ctx.pub_key;

    Ok(TlsContext {
        acceptor,
        public_key,
    })
}

/// Extract the raw `subjectPublicKey` bytes from a certificate.
///
/// The `CredSSP` protocol compares the raw bytes of the `subjectPublicKey`
/// BIT STRING from the server's TLS certificate. Both the server and client
/// must agree on this value. This matches the extraction in
/// `ironrdp_server::helper::TlsIdentityCtx` and the sspi crate's
/// `raw_peer_public_key()`.
fn extract_public_key(cert_der: &CertificateDer<'_>) -> Vec<u8> {
    use x509_cert::der::Decode as _;

    let cert = x509_cert::Certificate::from_der(cert_der)
        .expect("failed to parse self-generated certificate DER");

    cert.tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .as_bytes()
        .expect("subject public key BIT STRING is not byte-aligned")
        .to_owned()
}

fn make_acceptor(
    certs: &[CertificateDer<'static>],
    key: PrivateKeyDer<'static>,
) -> Result<TlsAcceptor> {
    let mut server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs.to_vec(), key)
        .context("bad certificate/key")?;

    // Only enable TLS key logging in debug builds (for Wireshark analysis).
    // In release builds this is a security risk as it leaks session keys.
    #[cfg(debug_assertions)]
    {
        server_config.key_log = Arc::new(rustls::KeyLogFile::new());
    }

    Ok(TlsAcceptor::from(Arc::new(server_config)))
}
