use sha2::{Digest, Sha256};
use tonic::{
    Request, Status,
    transport::server::{TcpConnectInfo, TlsConnectInfo},
};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateIdentity {
    pub sha256: String,
    pub not_before: u64,
    pub not_after: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportIdentity {
    Unix { uid: u32, gid: u32 },
    MutualTls(CertificateIdentity),
}
/// Extract only transport-injected identity, never caller metadata or self-reported fingerprints.
/// The caller must still match enrollment, installation, generation and current revocation state.
pub fn transport_identity<T>(request: &Request<T>, now: u64) -> Result<TransportIdentity, Status> {
    #[cfg(unix)]
    if let Some(info) = request
        .extensions()
        .get::<tonic::transport::server::UdsConnectInfo>()
    {
        let credential = info
            .peer_cred
            .as_ref()
            .ok_or_else(|| Status::unauthenticated("Unix peer credentials required"))?;
        return Ok(TransportIdentity::Unix {
            uid: credential.uid(),
            gid: credential.gid(),
        });
    }
    if let Some(info) = request.extensions().get::<TlsConnectInfo<TcpConnectInfo>>() {
        let chain = info
            .peer_certs()
            .ok_or_else(|| Status::unauthenticated("Client certificate required"))?;
        let leaf = chain
            .first()
            .ok_or_else(|| Status::unauthenticated("Client certificate required"))?;
        return certificate_identity(leaf.as_ref(), now).map(TransportIdentity::MutualTls);
    }
    Err(Status::unauthenticated("Authenticated transport required"))
}
/// Parse a leaf already authenticated by the TLS transport, and enforce validity on every RPC.
/// This function alone does not verify certificate signatures, enrollment or authorization.
pub fn certificate_identity(der: &[u8], now: u64) -> Result<CertificateIdentity, Status> {
    let denied = || Status::unauthenticated("Current client certificate required");
    if der.len() > 65536 {
        return Err(denied());
    }
    let (remaining, certificate) =
        x509_parser::parse_x509_certificate(der).map_err(|_| denied())?;
    let not_before =
        u64::try_from(certificate.validity().not_before.timestamp()).map_err(|_| denied())?;
    let not_after =
        u64::try_from(certificate.validity().not_after.timestamp()).map_err(|_| denied())?;
    if !remaining.is_empty() || now < not_before || now >= not_after {
        return Err(denied());
    }
    Ok(CertificateIdentity {
        sha256: format!("{:x}", Sha256::digest(der)),
        not_before,
        not_after,
    })
}
