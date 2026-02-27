//! TLS support for the wsh server.
//!
//! Loads PEM-encoded certificate chains and private keys, builds a rustls
//! `ServerConfig`, and wraps it in a `TlsAcceptor` for use with the manual
//! accept loop in `run_server()`.

use std::path::Path;
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;

/// Errors that can occur when loading TLS configuration.
#[derive(Debug)]
pub enum TlsError {
    /// Failed to read the certificate file.
    CertRead(std::io::Error),
    /// Failed to read the private key file.
    KeyRead(std::io::Error),
    /// No certificates found in the PEM file.
    NoCerts,
    /// No private key found in the PEM file.
    NoKey,
    /// Failed to build the TLS server configuration.
    Config(tokio_rustls::rustls::Error),
}

impl std::fmt::Display for TlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CertRead(e) => write!(f, "failed to read TLS certificate file: {}", e),
            Self::KeyRead(e) => write!(f, "failed to read TLS key file: {}", e),
            Self::NoCerts => write!(f, "no certificates found in PEM file"),
            Self::NoKey => write!(f, "no private key found in PEM file"),
            Self::Config(e) => write!(f, "failed to build TLS config: {}", e),
        }
    }
}

impl std::error::Error for TlsError {}

/// Load TLS certificate chain and private key from PEM files, returning a
/// `TlsAcceptor` ready for use with `tokio_rustls`.
pub fn load_tls_config(cert_path: &Path, key_path: &Path) -> Result<TlsAcceptor, TlsError> {
    // Read certificate chain
    let cert_data = std::fs::read(cert_path).map_err(TlsError::CertRead)?;
    let certs: Vec<_> = rustls_pemfile::certs(&mut &cert_data[..])
        .collect::<Result<Vec<_>, _>>()
        .map_err(TlsError::CertRead)?;
    if certs.is_empty() {
        return Err(TlsError::NoCerts);
    }

    // Read private key (try PKCS8, RSA, and EC formats)
    let key_data = std::fs::read(key_path).map_err(TlsError::KeyRead)?;
    let key = rustls_pemfile::private_key(&mut &key_data[..])
        .map_err(TlsError::KeyRead)?
        .ok_or(TlsError::NoKey)?;

    // Ensure a CryptoProvider is installed. This is idempotent if already set.
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();

    let config = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(TlsError::Config)?;

    Ok(TlsAcceptor::from(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_nonexistent_cert_returns_error() {
        let result = load_tls_config(Path::new("/nonexistent/cert.pem"), Path::new("/nonexistent/key.pem"));
        assert!(matches!(result, Err(TlsError::CertRead(_))));
    }

    #[test]
    fn load_empty_cert_returns_no_certs() {
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("cert.pem");
        let key_path = dir.path().join("key.pem");
        std::fs::write(&cert_path, "").unwrap();
        std::fs::write(&key_path, "").unwrap();

        let result = load_tls_config(&cert_path, &key_path);
        assert!(matches!(result, Err(TlsError::NoCerts)));
    }

    #[test]
    fn load_valid_self_signed_cert() {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("cert.pem");
        let key_path = dir.path().join("key.pem");

        std::fs::write(&cert_path, cert.cert.pem()).unwrap();
        std::fs::write(&key_path, cert.key_pair.serialize_pem()).unwrap();

        let result = load_tls_config(&cert_path, &key_path);
        assert!(result.is_ok(), "valid self-signed cert should load: {:?}", result.err());
    }

    #[test]
    fn load_cert_without_key_returns_no_key() {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("cert.pem");
        let key_path = dir.path().join("key.pem");

        std::fs::write(&cert_path, cert.cert.pem()).unwrap();
        std::fs::write(&key_path, "not a key").unwrap();

        let result = load_tls_config(&cert_path, &key_path);
        assert!(matches!(result, Err(TlsError::NoKey)));
    }
}
