//! The operator's additional public CA bundle augments system-independent WebPKI roots.
//! Hostname verification is never disabled, including for private runtime origins.
use crate::{Error, Result};
use std::sync::Arc;
#[derive(Clone)]
pub struct Trust {
    certificates: Vec<reqwest::Certificate>,
    websocket: Arc<rustls::ClientConfig>,
}
impl Trust {
    pub fn from_pem(pem: Option<&[u8]>) -> Result<Self> {
        let invalid = || Error::new(503, "Invalid runtime trust bundle.");
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut certificates = Vec::new();
        if let Some(pem) = pem {
            if pem.is_empty() || pem.len() > 1024 * 1024 {
                return Err(invalid());
            }
            for item in rustls_pemfile::read_all(&mut std::io::Cursor::new(pem)) {
                let rustls_pemfile::Item::X509Certificate(certificate) =
                    item.map_err(|_| invalid())?
                else {
                    return Err(invalid());
                };
                if certificates.len() >= 32 {
                    return Err(invalid());
                }
                certificates.push(
                    reqwest::Certificate::from_der(certificate.as_ref()).map_err(|_| invalid())?,
                );
                roots.add(certificate).map_err(|_| invalid())?;
            }
            if certificates.is_empty() {
                return Err(invalid());
            }
        }
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| invalid())?
        .with_root_certificates(roots)
        .with_no_client_auth();
        Ok(Self {
            certificates,
            websocket: Arc::new(config),
        })
    }
    pub fn http(&self, mut builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
        for certificate in &self.certificates {
            builder = builder.add_root_certificate(certificate.clone());
        }
        builder
    }
    pub fn websocket(&self) -> tokio_tungstenite::Connector {
        tokio_tungstenite::Connector::Rustls(self.websocket.clone())
    }
}
