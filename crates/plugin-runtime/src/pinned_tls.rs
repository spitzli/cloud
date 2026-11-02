//! Outbound remote runtime transport verifies both normal PKI and the enrolled leaf identity.
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Cursor},
    sync::Arc,
    time::Duration,
};
use tonic::transport::{Channel, Endpoint};
/// Operator-owned PEM bytes. Deliberately not Debug: the client key must never enter diagnostics.
pub struct PinnedMutualTls {
    pub certificate: Vec<u8>,
    pub private_key: Vec<u8>,
    pub authority: Vec<u8>,
    pub server_certificate_sha256: String,
}
#[derive(Debug)]
struct PinnedVerifier {
    inner: Arc<dyn ServerCertVerifier>,
    sha256: [u8; 32],
}
impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        leaf: &CertificateDer<'_>,
        chain: &[CertificateDer<'_>],
        name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let verified = self
            .inner
            .verify_server_cert(leaf, chain, name, ocsp, now)?;
        if Sha256::digest(leaf.as_ref())[..] != self.sha256 {
            return Err(rustls::Error::General(
                "Runtime certificate is not enrolled".into(),
            ));
        }
        Ok(verified)
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}
pub async fn pinned_mtls_channel(
    origin: String,
    domain: String,
    tls: PinnedMutualTls,
) -> Result<Channel, Box<dyn std::error::Error + Send + Sync>> {
    Ok(pinned_mtls_authenticated(origin, domain, tls).await?.0)
}
pub async fn pinned_mtls_authenticated(
    origin: String,
    domain: String,
    tls: PinnedMutualTls,
) -> Result<(Channel, crate::PeerIdentity), Box<dyn std::error::Error + Send + Sync>> {
    let (channel, peer) = pinned_connection(origin, domain, tls, false).await?;
    Ok((channel, peer.ok_or("Runtime peer certificate unavailable")?))
}
/// Validates local trust material now; every deferred connection still verifies PKI and leaf pin.
pub async fn pinned_mtls_channel_lazy(
    origin: String,
    domain: String,
    tls: PinnedMutualTls,
) -> Result<Channel, Box<dyn std::error::Error + Send + Sync>> {
    Ok(pinned_connection(origin, domain, tls, true).await?.0)
}
async fn pinned_connection(
    origin: String,
    domain: String,
    tls: PinnedMutualTls,
    lazy: bool,
) -> Result<(Channel, Option<crate::PeerIdentity>), Box<dyn std::error::Error + Send + Sync>> {
    let url = url::Url::parse(&origin)?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || [
            tls.certificate.len(),
            tls.private_key.len(),
            tls.authority.len(),
        ]
        .iter()
        .any(|n| *n > 65536)
    {
        return Err("Explicit pinned HTTPS runtime required".into());
    }
    let host = url.host_str().ok_or("Runtime host required")?.to_owned();
    let port = url.port_or_known_default().ok_or("Runtime port required")?;
    let name = ServerName::try_from(domain)?;
    let sha: [u8; 32] = hex::decode(&tls.server_certificate_sha256)?
        .try_into()
        .map_err(|_| "Invalid runtime certificate pin")?;
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut Cursor::new(&tls.authority)) {
        roots.add(cert?)?;
    }
    if roots.is_empty() {
        return Err("Runtime certificate authority required".into());
    }
    let certificates =
        rustls_pemfile::certs(&mut Cursor::new(&tls.certificate)).collect::<Result<Vec<_>, _>>()?;
    if certificates.is_empty() {
        return Err("Runtime client certificate required".into());
    }
    let key = rustls_pemfile::private_key(&mut Cursor::new(tls.private_key))?
        .ok_or("Runtime client key required")?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .build()?;
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedVerifier {
            inner: verifier,
            sha256: sha,
        }))
        .with_client_auth_cert(certificates, key)?;
    config.alpn_protocols = vec![b"h2".to_vec()];
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    // Tonic otherwise wraps custom connectors in a second TLS handshake. The internal HTTP URI
    // selects its already-secured IO path; this connector always completes verified mutual TLS.
    let mut logical = url.clone();
    logical
        .set_scheme("http")
        .map_err(|_| "Invalid runtime origin")?;
    let endpoint = Endpoint::from_shared(logical.to_string())?
        .origin(origin.parse()?)
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(55));
    let observed = Arc::new(std::sync::Mutex::new(None));
    let observed_connection = observed.clone();
    let service = tower::service_fn(move |_: tonic::codegen::http::Uri| {
        let connector = connector.clone();
        let host = host.clone();
        let name = name.clone();
        let observed = observed_connection.clone();
        async move {
            let socket = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
            let stream = connector.connect(name, socket).await?;
            if stream.get_ref().1.alpn_protocol() != Some(b"h2") {
                return Err(io::Error::other("Runtime HTTP/2 required"));
            }
            let peer = stream
                .get_ref()
                .1
                .peer_certificates()
                .ok_or_else(|| io::Error::other("Runtime peer certificate required"))?
                .iter()
                .map(|c| c.as_ref().to_vec())
                .collect::<Vec<_>>();
            *observed
                .lock()
                .map_err(|_| io::Error::other("Runtime peer state unavailable"))? = Some(peer);
            Ok::<_, io::Error>(hyper_util::rt::TokioIo::new(stream))
        }
    });
    if lazy {
        return Ok((endpoint.connect_with_connector_lazy(service), None));
    }
    let channel = endpoint.connect_with_connector(service).await?;
    let certificates = observed
        .lock()
        .map_err(|_| "Runtime peer state unavailable")?
        .clone()
        .ok_or("Runtime peer certificate unavailable")?;
    Ok((
        channel,
        Some(crate::PeerIdentity::MutualTls { certificates }),
    ))
}
pub async fn connect_pinned_mtls(
    origin: String,
    domain: String,
    tls: PinnedMutualTls,
) -> Result<
    crate::wire::plugin_runtime_client::PluginRuntimeClient<Channel>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    Ok(
        crate::wire::plugin_runtime_client::PluginRuntimeClient::new(
            pinned_mtls_channel(origin, domain, tls).await?,
        )
        .max_decoding_message_size(webdock_plugin_protocol::MANIFEST_BYTES + 1024)
        .max_encoding_message_size(128 * 1024),
    )
}
