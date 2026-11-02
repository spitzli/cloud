//! Operator-owned native plugin routing. URLs and private keys never come from action payloads.
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use webdock_plugin_runtime::{PinnedMutualTls, wire::plugin_runtime_client::PluginRuntimeClient};
pub type ConfigurationError = Box<dyn std::error::Error + Send + Sync>;
pub fn installation_secret() -> Result<String, ConfigurationError> {
    std::env::var("WEBDOCK_NATIVE_PLUGIN_ENCRYPTION_KEY")
        .or_else(|_| std::env::var("WEBDOCK_AUTH_SECRET"))
        .or_else(|_| std::env::var("BETTER_AUTH_SECRET"))
        .map_err(|_| "Missing native installation encryption secret".into())
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginConfig {
    #[serde(default)]
    pub export_sources: bool,
    #[serde(default)]
    pub storage: Option<StorageGatewayConfig>,
    pub host_bindings: PathBuf,
    pub invocation_signing_key: PathBuf,
    pub connections: PathBuf,
    pub control: ControlListener,
    #[serde(default)]
    pub installation_catalog: Option<PathBuf>,
    #[serde(default)]
    pub repository: Option<RepositoryConfig>,
    #[serde(default)]
    pub mail: Option<MailConfig>,
    #[serde(default)]
    pub mcp_client: Option<McpClientConfig>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageGatewayConfig {
    pub staging_directory: PathBuf,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryConfig {
    pub bindings: PathBuf,
    pub signing_key: PathBuf,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MailConfig {
    pub signing_key: PathBuf,
    pub worker_id: String,
    pub worker_generation: u64,
}
pub type McpClientConfig = crate::resource_auth::ResourceClientConfig;
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ControlListener {
    Unix {
        path: PathBuf,
    },
    MutualTls {
        address: std::net::SocketAddr,
        certificate: PathBuf,
        private_key: PathBuf,
        authority: PathBuf,
    },
}
#[derive(Clone, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeTransport {
    Unix {
        path: PathBuf,
    },
    MutualTls {
        origin: String,
        domain: String,
        certificate: PathBuf,
        private_key: PathBuf,
        authority: PathBuf,
        server_certificate_sha256: String,
    },
}
#[derive(Clone, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeConnection {
    pub generation: u64,
    pub transport: RuntimeTransport,
    #[serde(default)]
    pub ui_origin: Option<String>,
}
#[derive(Clone)]
pub struct RuntimeConnections {
    path: PathBuf,
    managed: Option<std::sync::Arc<webdock_plugin_host::ManagedBindings>>,
}
impl PluginConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigurationError> {
        let value: Self = serde_json::from_slice(&crate::config::read_private_file(path)?)?;
        for p in [
            &value.host_bindings,
            &value.invocation_signing_key,
            &value.connections,
        ] {
            if !p.is_absolute() {
                return Err("Absolute plugin configuration paths required".into());
            }
        }
        if value
            .mail
            .as_ref()
            .is_some_and(|mail| !mail.signing_key.is_absolute())
        {
            return Err("Absolute Mail signing-key path required".into());
        }
        if value
            .installation_catalog
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
        {
            return Err("Absolute installation catalog path required".into());
        }
        Ok(value)
    }
}
pub fn signing_key(path: &Path) -> Result<ed25519_dalek::SigningKey, ConfigurationError> {
    let value = crate::config::read_private_file(path)?;
    let value = std::str::from_utf8(&value)?.trim();
    if value.len() != 64 {
        return Err("Invalid plugin signing key".into());
    }
    let bytes: [u8; 32] = hex::decode(value)?
        .try_into()
        .map_err(|_| "Invalid plugin signing key")?;
    Ok(ed25519_dalek::SigningKey::from_bytes(&bytes))
}
fn pem(path: &Path) -> Result<Vec<u8>, ConfigurationError> {
    let value = crate::config::read_private_file(path)?;
    if value.is_empty() || value.len() > 65536 {
        return Err("Invalid plugin transport material".into());
    }
    Ok(value)
}
impl RuntimeConnections {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            managed: None,
        }
    }
    pub fn with_managed(
        mut self,
        managed: std::sync::Arc<webdock_plugin_host::ManagedBindings>,
    ) -> Self {
        self.managed = Some(managed);
        self
    }
    fn file(&self, plugin: &str) -> Result<Option<RuntimeConnection>, ConfigurationError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct File {
            runtimes: BTreeMap<String, RuntimeConnection>,
        }
        let mut config: File =
            serde_json::from_slice(&crate::config::read_private_file(&self.path)?)?;
        Ok(config.runtimes.remove(plugin))
    }
    pub fn load(&self, plugin: &str) -> Result<RuntimeConnection, ConfigurationError> {
        Self::checked(self.file(plugin)?.ok_or("Plugin runtime is not routed")?)
    }
    pub async fn load_generation(
        &self,
        plugin: &str,
        generation: u64,
    ) -> Result<RuntimeConnection, ConfigurationError> {
        let file = self.file(plugin)?;
        let managed = match &self.managed {
            Some(bindings) => bindings
                .managed_endpoint(plugin, generation)
                .await?
                .map(serde_json::from_value::<RuntimeConnection>)
                .transpose()?,
            None => None,
        };
        if file.is_some() && managed.is_some() {
            return Err("Managed and file runtime routing collide".into());
        }
        Self::checked(managed.or(file).ok_or("Plugin runtime is not routed")?)
    }
    fn checked(value: RuntimeConnection) -> Result<RuntimeConnection, ConfigurationError> {
        if value.generation == 0 || value.generation > 9_007_199_254_740_991 {
            return Err("Invalid plugin generation".into());
        }
        if let Some(origin) = &value.ui_origin {
            let url = url::Url::parse(origin)?;
            if url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || url.path() != "/"
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err("Invalid plugin UI origin".into());
            }
        }
        Ok(value)
    }
}
impl RuntimeConnection {
    pub async fn connect_authenticated(
        &self,
    ) -> Result<
        (
            PluginRuntimeClient<tonic::transport::Channel>,
            webdock_plugin_runtime::PeerIdentity,
        ),
        ConfigurationError,
    > {
        match &self.transport {
            RuntimeTransport::Unix { path } => Ok((
                webdock_plugin_runtime::connect_unix(path).await?,
                webdock_plugin_runtime::PeerIdentity::Unix {
                    uid: rustix::process::geteuid().as_raw(),
                },
            )),
            RuntimeTransport::MutualTls {
                origin,
                domain,
                certificate,
                private_key,
                authority,
                server_certificate_sha256,
            } => {
                let (channel, peer) = webdock_plugin_runtime::pinned_mtls_authenticated(
                    origin.clone(),
                    domain.clone(),
                    PinnedMutualTls {
                        certificate: pem(certificate)?,
                        private_key: pem(private_key)?,
                        authority: pem(authority)?,
                        server_certificate_sha256: server_certificate_sha256.clone(),
                    },
                )
                .await?;
                Ok((
                    PluginRuntimeClient::new(channel)
                        .max_decoding_message_size(256 * 1024 + 1024)
                        .max_encoding_message_size(128 * 1024),
                    peer,
                ))
            }
        }
    }

    pub async fn connect(
        &self,
    ) -> Result<PluginRuntimeClient<tonic::transport::Channel>, ConfigurationError> {
        match &self.transport {
            RuntimeTransport::Unix { path } => webdock_plugin_runtime::connect_unix(path).await,
            RuntimeTransport::MutualTls {
                origin,
                domain,
                certificate,
                private_key,
                authority,
                server_certificate_sha256,
            } => {
                webdock_plugin_runtime::connect_pinned_mtls(
                    origin.clone(),
                    domain.clone(),
                    PinnedMutualTls {
                        certificate: pem(certificate)?,
                        private_key: pem(private_key)?,
                        authority: pem(authority)?,
                        server_certificate_sha256: server_certificate_sha256.clone(),
                    },
                )
                .await
            }
        }
    }
}
impl ControlListener {
    pub fn tls(&self) -> Result<Option<webdock_plugin_runtime::MutualTls>, ConfigurationError> {
        match self {
            Self::Unix { .. } => Ok(None),
            Self::MutualTls {
                certificate,
                private_key,
                authority,
                ..
            } => Ok(Some(webdock_plugin_runtime::MutualTls {
                identity: tonic::transport::Identity::from_pem(
                    pem(certificate)?,
                    pem(private_key)?,
                ),
                peer_ca: tonic::transport::Certificate::from_pem(pem(authority)?),
            })),
        }
    }
}

/// Private listeners reuse the same native identity/OAuth/browser policy as HTTP Core without
/// constructing public account routes or performing bootstrap/enrollment writes.
pub struct AuthorityServices {
    pub identity: std::sync::Arc<webdock_identity::IdentityService>,
    pub oauth: std::sync::Arc<webdock_oauth::OAuthService>,
    pub browser: Option<std::sync::Arc<webdock_browser_session::BrowserSessions>>,
}
pub async fn authority_services(
    auth: sea_orm::DatabaseConnection,
    ids: std::sync::Arc<webdock_persistence::IdAllocator>,
    config: &webdock_core::config::PlatformConfig,
) -> Result<AuthorityServices, ConfigurationError> {
    use std::sync::Arc;
    let secret = std::env::var("WEBDOCK_AUTH_SECRET")
        .or_else(|_| std::env::var("BETTER_AUTH_SECRET"))
        .map_err(|_| "Missing identity secret")?;
    let keys = || {
        webdock_identity::SecretKeys::new(
            std::collections::BTreeMap::from([(1, secret.clone())]),
            1,
            Some(secret.clone()),
        )
    };
    let identity = Arc::new(
        webdock_identity::IdentityService::new(
            auth.clone(),
            webdock_identity::CookiePolicy::new(
                config.auth_origin.starts_with("https://"),
                secret.clone(),
            )?,
        )
        .with_secret_keys(keys()?)
        .with_allocator(ids.clone())
        .with_account_origin(url::Url::parse(&config.auth_origin)?)?,
    );
    let oauth = Arc::new(webdock_oauth::OAuthService::new(
        auth.clone(),
        identity.clone(),
        ids,
        Arc::new(keys()?),
        webdock_oauth::OAuthConfig::new(
            &format!("{}/api/auth", config.auth_origin),
            &std::env::var("WEBDOCK_MCP_RESOURCE")
                .unwrap_or_else(|_| format!("{}/api/mcp", config.studio_origin)),
            std::env::var("WEBDOCK_STUDIO_CLIENT_ID")
                .or_else(|_| std::env::var("WEBDOCK_SSO_CLIENT_ID"))
                .ok(),
        )?,
    ));
    let browser = if let Ok(client) = std::env::var("WEBDOCK_SSO_CLIENT_ID") {
        let secret = std::env::var("WEBDOCK_SSO_CLIENT_SECRET")
            .map_err(|_| "Missing first-party client secret")?;
        let cookie = std::env::var("WEBDOCK_SSO_COOKIE_SECRET")
            .map_err(|_| "Missing first-party cookie secret")?;
        let options = webdock_browser_session::BrowserConfig::new(
            config.console_origin(),
            &format!("{}/api/auth", config.auth_origin),
            client,
            secret,
        )?
        .with_additional_origins(std::slice::from_ref(&config.studio_origin))?;
        Some(Arc::new(webdock_browser_session::BrowserSessions::new(
            auth,
            identity.clone(),
            oauth.clone(),
            Arc::new(keys()?),
            &cookie,
            options,
        )?))
    } else {
        None
    };
    Ok(AuthorityServices {
        identity,
        oauth,
        browser,
    })
}
