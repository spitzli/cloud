//! Private operator-configured transports shared by product executables.
use crate::{Error, read_file};
use serde::Deserialize;
use std::{net::SocketAddr, path::PathBuf};
use webdock_plugin_runtime::{HostClient, MutualTls, PinnedMutualTls};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListenTls {
    pub address: SocketAddr,
    pub certificate: PathBuf,
    pub private_key: PathBuf,
    pub authority: PathBuf,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreTls {
    pub origin: String,
    pub domain: String,
    pub certificate: PathBuf,
    pub private_key: PathBuf,
    pub authority: PathBuf,
    pub server_certificate_sha256: String,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeTransport {
    pub listen: Option<PathBuf>,
    pub listen_tls: Option<ListenTls>,
    pub core: Option<PathBuf>,
    pub core_tls: Option<CoreTls>,
}
fn paths(paths: &[&std::path::Path]) -> bool {
    paths.iter().all(|p| p.is_absolute())
}
impl RuntimeTransport {
    pub fn validate(&self) -> Result<(), Error> {
        if self.listen.is_some() == self.listen_tls.is_some()
            || self.core.is_some() == self.core_tls.is_some()
            || self.listen.as_ref().is_some_and(|p| !p.is_absolute())
            || self.core.as_ref().is_some_and(|p| !p.is_absolute())
        {
            return Err("exactly one absolute listener and callback required".into());
        }
        if let Some(tls) = &self.listen_tls
            && !paths(&[&tls.certificate, &tls.private_key, &tls.authority])
        {
            return Err("absolute TLS paths required".into());
        }
        if let Some(tls) = &self.core_tls {
            let url = url::Url::parse(&tls.origin).map_err(|_| "invalid Core TLS origin")?;
            if url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || url.path() != "/"
                || url.query().is_some()
                || url.fragment().is_some()
                || url.host_str() != Some(tls.domain.as_str())
                || tls.server_certificate_sha256.len() != 64
                || !tls
                    .server_certificate_sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit())
                || !paths(&[&tls.certificate, &tls.private_key, &tls.authority])
            {
                return Err("exact pinned Core HTTPS origin required".into());
            }
        }
        Ok(())
    }
    pub async fn core_client(&self) -> Result<HostClient, Error> {
        self.core_client_with_mode(false).await
    }
    /// Deferred callback availability must not prevent candidate discovery from listening.
    pub async fn core_client_lazy(&self) -> Result<HostClient, Error> {
        self.core_client_with_mode(true).await
    }
    async fn core_client_with_mode(&self, lazy: bool) -> Result<HostClient, Error> {
        self.validate()?;
        let channel = match (&self.core, &self.core_tls) {
            (Some(path), None) if lazy => webdock_plugin_runtime::unix_channel_lazy(path)?,
            (Some(path), None) => webdock_plugin_runtime::unix_channel(path.clone()).await?,
            (None, Some(tls)) => {
                let material = PinnedMutualTls {
                    certificate: read_file(&tls.certificate, false)?,
                    private_key: read_file(&tls.private_key, true)?,
                    authority: read_file(&tls.authority, false)?,
                    server_certificate_sha256: tls.server_certificate_sha256.clone(),
                };
                if lazy {
                    webdock_plugin_runtime::pinned_mtls_channel_lazy(
                        tls.origin.clone(),
                        tls.domain.clone(),
                        material,
                    )
                    .await?
                } else {
                    webdock_plugin_runtime::pinned_mtls_channel(
                        tls.origin.clone(),
                        tls.domain.clone(),
                        material,
                    )
                    .await?
                }
            }
            _ => return Err("Core transport required".into()),
        };
        Ok(HostClient::new(channel))
    }
}
impl ListenTls {
    pub fn material(&self) -> Result<MutualTls, Error> {
        Ok(MutualTls {
            identity: tonic::transport::Identity::from_pem(
                read_file(&self.certificate, false)?,
                read_file(&self.private_key, true)?,
            ),
            peer_ca: tonic::transport::Certificate::from_pem(read_file(&self.authority, false)?),
        })
    }
}
