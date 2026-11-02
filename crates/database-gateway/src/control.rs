//! Confidential API-to-gateway access contract. These structs are never browser response DTOs.
use crate::{Error, Result, policy::Profile};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use webdock_core::id::RecordId;
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeAccess {
    #[serde(rename = "scopeID")]
    pub scope_id: RecordId,
    pub subject: String,
    #[serde(rename = "connectionID")]
    pub connection_id: String,
    pub profile: Profile,
    pub runtime_origin: String,
    pub proxy_secret: String,
    pub expires_at: DateTime<Utc>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedAccess {
    pub kind: ManagedKind,
    #[serde(rename = "scopeID")]
    pub scope_id: RecordId,
    pub subject: String,
    #[serde(rename = "connectionID")]
    pub connection_id: String,
    pub profile: Profile,
    pub expires_at: DateTime<Utc>,
    pub revision: String,
    pub database: DatabaseCredentials,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ManagedKind {
    Managed,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseCredentials {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: String,
    pub ca_certificate: String,
}
#[derive(Clone, Deserialize)]
#[serde(untagged)]
pub enum Access {
    Registered(RuntimeAccess),
    Managed(ManagedAccess),
}
impl Access {
    pub fn validate(&self) -> Result<()> {
        let (subject, connection, expires) = match self {
            Self::Registered(value) => {
                if normalize_origin(&value.runtime_origin)? != value.runtime_origin
                    || value.proxy_secret.len() < 32
                    || value.proxy_secret.len() > 512
                {
                    return Err(invalid_access());
                }
                (&value.subject, &value.connection_id, value.expires_at)
            }
            Self::Managed(value) => {
                let db = &value.database;
                if value.profile != Profile::Schema
                    || value.revision.is_empty()
                    || value.revision.len() > 128
                    || db.host.is_empty()
                    || db.host.len() > 253
                    || db.port == 0
                    || db.database.is_empty()
                    || db.database.len() > 128
                    || db.username.is_empty()
                    || db.username.len() > 128
                    || db.password.is_empty()
                    || db.password.len() > 4096
                    || db.ca_certificate.is_empty()
                    || db.ca_certificate.len() > 65536
                {
                    return Err(invalid_access());
                }
                (&value.subject, &value.connection_id, value.expires_at)
            }
        };
        if subject.is_empty()
            || subject.len() > 128
            || connection.is_empty()
            || connection.len() > 128
            || expires <= Utc::now()
        {
            return Err(invalid_access());
        }
        Ok(())
    }
    pub fn scope_id(&self) -> &str {
        match self {
            Self::Registered(v) => v.scope_id.as_str(),
            Self::Managed(v) => v.scope_id.as_str(),
        }
    }
    pub fn expires_at(&self) -> DateTime<Utc> {
        match self {
            Self::Registered(v) => v.expires_at,
            Self::Managed(v) => v.expires_at,
        }
    }
}
fn invalid_access() -> Error {
    Error::new(401, "Database runtime is not authorized.")
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Launch {
    pub token: String,
    #[serde(rename = "scopeID")]
    pub scope_id: RecordId,
    pub expires_at: DateTime<Utc>,
}
#[async_trait::async_trait]
pub trait Control: Send + Sync + 'static {
    async fn exchange(&self, code: &str) -> Result<Launch>;
    async fn inspect(&self, token: &str) -> Result<Access>;
    async fn logout(&self, token: &str) -> Result<()>;
}
pub fn normalize_origin(raw: &str) -> Result<String> {
    let invalid = || Error::new(503, "Use an HTTPS origin without credentials or a path.");
    if raw.len() > 300
        || raw.contains('\\')
        || raw
            .bytes()
            .any(|v| v.is_ascii_control() || v.is_ascii_whitespace())
    {
        return Err(invalid());
    }
    let url = url::Url::parse(raw).map_err(|_| invalid())?;
    let local = url.scheme() == "http"
        && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if (!local && url.scheme() != "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(url.origin().ascii_serialization())
}
pub struct HttpControl {
    origin: String,
    secret: String,
    client: reqwest::Client,
}
impl HttpControl {
    pub fn new(origin: &str, secret: String) -> Result<Self> {
        Self::new_with_trust(origin, secret, &crate::trust::Trust::from_pem(None)?)
    }
    pub fn new_with_trust(
        origin: &str,
        secret: String,
        trust: &crate::trust::Trust,
    ) -> Result<Self> {
        if secret.len() < 32 || secret.len() > 4096 {
            return Err(Error::new(503, "Gateway authorization is not configured."));
        }
        let client = trust
            .http(reqwest::Client::builder())
            .no_proxy()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|_| Error::new(503, "Gateway transport unavailable."))?;
        Ok(Self {
            origin: normalize_origin(origin)?,
            secret,
            client,
        })
    }
    async fn call(&self, body: Value) -> Result<Value> {
        let response = self
            .client
            .post(format!("{}/api/databases/gateway", self.origin))
            .bearer_auth(&self.secret)
            .json(&body)
            .send()
            .await
            .map_err(|_| Error::new(503, "Database authorization unavailable."))?;
        if !response.status().is_success() {
            return Err(Error::new(401, "Database access expired or was revoked."));
        }
        let value = bounded_json(response, 131072).await?;
        value
            .get("data")
            .cloned()
            .ok_or(Error::new(502, "Invalid database authorization response."))
    }
}
#[async_trait::async_trait]
impl Control for HttpControl {
    async fn exchange(&self, code: &str) -> Result<Launch> {
        serde_json::from_value(self.call(json!({"action":"exchange","code":code})).await?)
            .map_err(|_| Error::new(502, "Invalid session response."))
    }
    async fn inspect(&self, token: &str) -> Result<Access> {
        let value: Access =
            serde_json::from_value(self.call(json!({"action":"inspect","token":token})).await?)
                .map_err(|_| invalid_access())?;
        value.validate()?;
        Ok(value)
    }
    async fn logout(&self, token: &str) -> Result<()> {
        self.call(json!({"action":"logout","token":token})).await?;
        Ok(())
    }
}
pub(crate) async fn bounded_json(response: reqwest::Response, maximum: usize) -> Result<Value> {
    use futures_util::StreamExt;
    if response
        .content_length()
        .is_some_and(|size| size > maximum as u64)
    {
        return Err(Error::new(
            413,
            "Database result is too large. Use pagination.",
        ));
    }
    let mut body = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|_| Error::new(502, "Database response unavailable."))?;
        if bytes.len().saturating_add(chunk.len()) > maximum {
            return Err(Error::new(
                413,
                "Database result is too large. Use pagination.",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::new(502, "Invalid database response."))
}
