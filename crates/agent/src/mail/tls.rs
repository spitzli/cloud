//! Capture a self-signed submission certificate only after Docker ownership and admin proof.
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use sha2::{Digest, Sha256};
use std::{net::Ipv4Addr, sync::Arc, time::Duration};
use webdock_supervisor::{ExecutionError, Result};
use x509_parser::prelude::FromDer;
#[derive(Debug)]
struct OwnedEndpoint {
    provider: Arc<rustls::crypto::CryptoProvider>,
}
impl ServerCertVerifier for OwnedEndpoint {
    fn verify_server_cert(
        &self,
        end: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let (_, cert) =
            x509_parser::certificate::X509Certificate::from_der(end.as_ref()).map_err(|_| {
                rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
            })?;
        let at = i64::try_from(now.as_secs())
            .ok()
            .and_then(|at| x509_parser::time::ASN1Time::from_timestamp(at).ok())
            .ok_or_else(|| rustls::Error::General("invalid certificate time".into()))?;
        if !cert.validity().is_valid_at(at) {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::Expired,
            ));
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
pub(super) async fn certificate(ip: Ipv4Addr) -> Result<String> {
    if !ip.is_private() {
        return Err(ExecutionError::Ownership);
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|_| ExecutionError::Runtime)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(OwnedEndpoint { provider }))
        .with_no_client_auth();
    tokio::time::timeout(Duration::from_secs(3), async {
        let stream = tokio::net::TcpStream::connect((ip, 2465))
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(ServerName::IpAddress(ip.into()), stream)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let cert = tls
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|v| v.first())
            .ok_or(ExecutionError::Runtime)?;
        Ok(Sha256::digest(cert.as_ref())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    })
    .await
    .map_err(|_| ExecutionError::Runtime)?
}
