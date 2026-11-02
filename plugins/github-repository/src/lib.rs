//! Independently installed GitHub repository provider. Core has no GitHub dependency.
use reqwest::{Client, Method, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashMap, time::Duration};
use webdock_repository_protocol::{
    self as protocol, ErrorCode, LeaseClaims, PluginError, Result, runtime::Handler,
};
fn denied() -> PluginError {
    PluginError::denied()
}
fn unavailable() -> PluginError {
    PluginError::new(
        ErrorCode::CapabilityUnavailable,
        "Repository provider unavailable",
    )
}
fn invalid() -> PluginError {
    PluginError::new(ErrorCode::InvalidInput, "Invalid repository request")
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveInput {
    #[serde(rename = "installationID")]
    installation_id: String,
    #[serde(rename = "repositoryID")]
    repository_id: String,
    reference: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArchiveInput {
    #[serde(rename = "installationID")]
    installation_id: String,
    #[serde(rename = "repositoryID")]
    repository_id: String,
    commit: String,
}
#[derive(Clone)]
pub struct GithubProvider {
    client: Client,
    app_id: u64,
    key: octocrab::auth::EncodingKey,
    api: url::Url,
    codeload: url::Url,
    archives: std::sync::Arc<tokio::sync::Semaphore>,
}
fn id(value: &str) -> Result<u64> {
    if value.is_empty()
        || value.starts_with('0')
        || value.len() > 16
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid());
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|v| *v > 0 && *v <= 9_007_199_254_740_991)
        .ok_or_else(invalid)
}
fn number(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| id(s).ok()))
        .filter(|v| *v > 0 && *v <= 9_007_199_254_740_991)
}
fn name(value: &Value) -> Result<&str> {
    let name = value.as_str().ok_or_else(denied)?;
    if name.is_empty()
        || name.len() > 100
        || name == "."
        || name == ".."
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(denied());
    }
    Ok(name)
}
fn commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl GithubProvider {
    pub fn new(app_id: u64, pem: &[u8]) -> Result<Self> {
        Self::configured(
            app_id,
            pem,
            "https://api.github.com/",
            "https://codeload.github.com/",
        )
    }
    fn configured(app_id: u64, pem: &[u8], api: &str, codeload: &str) -> Result<Self> {
        if app_id == 0 || app_id > 9_007_199_254_740_991 || pem.len() > 32768 {
            return Err(invalid());
        }
        let key = octocrab::auth::EncodingKey::from_rsa_pem(pem).map_err(|_| invalid())?;
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(5))
            .user_agent("webdock-github-repository/1")
            .build()
            .map_err(|_| unavailable())?;
        Ok(Self {
            client,
            app_id,
            key,
            api: url::Url::parse(api).map_err(|_| invalid())?,
            codeload: url::Url::parse(codeload).map_err(|_| invalid())?,
            archives: std::sync::Arc::new(tokio::sync::Semaphore::new(2)),
        })
    }
    #[cfg(feature = "local-provider-test")]
    pub fn loopback_test(app_id: u64, pem: &[u8], origin: &str) -> Result<Self> {
        let url = url::Url::parse(origin).map_err(|_| invalid())?;
        if url.scheme() != "http"
            || url.host_str() != Some("127.0.0.1")
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(invalid());
        }
        Self::configured(app_id, pem, origin, origin)
    }
    async fn body(mut response: reqwest::Response, limit: u64) -> Result<Vec<u8>> {
        if response
            .content_length()
            .is_some_and(|length| length > limit)
        {
            return Err(denied());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if (bytes.len() as u64).saturating_add(chunk.len() as u64) > limit {
                return Err(denied());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    async fn request(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> Result<reqwest::Response> {
        let url = self.api.join(path).map_err(|_| invalid())?;
        if url.origin() != self.api.origin()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(denied());
        }
        let mut request = self
            .client
            .request(method, url)
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28");
        if let Some(body) = body {
            request = request.json(&body);
        }
        request.send().await.map_err(|_| unavailable())
    }
    async fn json(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let response = self.request(method, path, token, body).await?;
        if !response.status().is_success() {
            return Err(denied());
        }
        serde_json::from_slice(&Self::body(response, 2_000_000).await?).map_err(|_| denied())
    }
    async fn token(
        &self,
        installation: &str,
        repository: &str,
        permissions: HashMap<String, String>,
    ) -> Result<String> {
        let installation = id(installation)?;
        let repository = id(repository)?;
        let jwt = octocrab::auth::create_jwt(octocrab::models::AppId(self.app_id), &self.key)
            .map_err(|_| unavailable())?;
        let current = self
            .json(
                Method::GET,
                &format!("/app/installations/{installation}"),
                &jwt,
                None,
            )
            .await?;
        if number(&current["id"]) != Some(installation)
            || number(&current["app_id"]) != Some(self.app_id)
            || !current["suspended_at"].is_null()
            || number(&current["account"]["id"]).is_none()
            || !["read", "write"]
                .contains(&current["permissions"]["contents"].as_str().unwrap_or(""))
            || current["permissions"]["checks"] != "write"
            || !["read", "write"]
                .contains(&current["permissions"]["metadata"].as_str().unwrap_or(""))
        {
            return Err(denied());
        }
        // The maintained SDK's typed request always receives an explicit single-repository scope.
        // Its typed response ignores unknown permission keys, so validate the raw response instead.
        let request = octocrab::models::apps::CreateInstallationAccessToken {
            repositories: None,
            repository_ids: Some(vec![octocrab::models::RepositoryId(repository)]),
            permissions: Some(permissions.clone()),
        };
        let value = self
            .json(
                Method::POST,
                &format!("/app/installations/{installation}/access_tokens"),
                &jwt,
                Some(serde_json::to_value(request).map_err(|_| invalid())?),
            )
            .await?;
        let token = value["token"].as_str().ok_or_else(denied)?;
        let returned: HashMap<String, String> =
            serde_json::from_value(value["permissions"].clone()).map_err(|_| denied())?;
        if token.is_empty()
            || token.len() > 8192
            || token.bytes().any(|b| b <= 32 || b >= 127)
            || returned != permissions
            || value["repositories"]
                .as_array()
                .is_none_or(|repos| repos.len() != 1 || number(&repos[0]["id"]) != Some(repository))
            || chrono::DateTime::parse_from_rfc3339(
                value["expires_at"].as_str().ok_or_else(denied)?,
            )
            .ok()
            .is_none_or(|expires| expires <= chrono::Utc::now())
        {
            return Err(denied());
        }
        Ok(token.into())
    }
    async fn repository(
        &self,
        installation: &str,
        repository: &str,
    ) -> Result<(String, String, String)> {
        let token = self
            .token(
                installation,
                repository,
                HashMap::from([
                    ("contents".into(), "read".into()),
                    ("metadata".into(), "read".into()),
                ]),
            )
            .await?;
        let repo = self
            .json(
                Method::GET,
                &format!("/repositories/{}", id(repository)?),
                &token,
                None,
            )
            .await?;
        if number(&repo["id"]) != Some(id(repository)?) {
            return Err(denied());
        }
        let owner = name(&repo["owner"]["login"])?;
        let name = name(&repo["name"])?;
        if repo["default_branch"]
            .as_str()
            .is_none_or(|v| v.is_empty() || v.len() > 255)
        {
            return Err(denied());
        }
        Ok((token, owner.into(), name.into()))
    }
    async fn resolve_ref(&self, input: ResolveInput) -> Result<Value> {
        if input.reference.is_empty()
            || input.reference.len() > 255
            || input.reference.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        let (token, owner, name) = self
            .repository(&input.installation_id, &input.repository_id)
            .await?;
        let mut url = self.api.clone();
        url.path_segments_mut()
            .map_err(|_| invalid())?
            .clear()
            .extend([
                "repos",
                &owner,
                &name,
                if commit(&input.reference) {
                    "commits"
                } else {
                    "branches"
                },
                &input.reference,
            ]);
        let value = self.json(Method::GET, url.path(), &token, None).await?;
        let sha = if commit(&input.reference) {
            if value["sha"] != input.reference {
                return Err(denied());
            }
            value["sha"].as_str()
        } else {
            if value["name"] != input.reference {
                return Err(denied());
            }
            value["commit"]["sha"].as_str()
        }
        .filter(|v| commit(v))
        .ok_or_else(denied)?;
        Ok(json!({"repositoryID":input.repository_id,"sha":sha}))
    }
    async fn source(&self, input: ArchiveInput) -> Result<Vec<u8>> {
        if !commit(&input.commit) {
            return Err(invalid());
        }
        let (token, owner, name) = self
            .repository(&input.installation_id, &input.repository_id)
            .await?;
        let response = self
            .request(
                Method::GET,
                &format!("/repos/{owner}/{name}/tarball/{}", input.commit),
                &token,
                None,
            )
            .await?;
        if response.status() != StatusCode::FOUND {
            return Err(denied());
        }
        let location = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(denied)?;
        let url = url::Url::parse(location).map_err(|_| denied())?;
        if url.origin() != self.codeload.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || location.len() > 8192
            || url.path() != format!("/{owner}/{name}/legacy.tar.gz/{}", input.commit)
        {
            return Err(denied());
        }
        // Credentials are never forwarded to the download origin, including when origins share a host.
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| unavailable())?;
        if response.status() != StatusCode::OK {
            return Err(denied());
        }
        let compressed = Self::body(response, protocol::SOURCE_BYTES).await?;
        let permit = self
            .archives
            .clone()
            .try_acquire_owned()
            .map_err(|_| unavailable())?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            archive::normalize(&compressed)
        })
        .await
        .map_err(|_| unavailable())?
    }
}
#[async_trait::async_trait]
impl Handler for GithubProvider {
    async fn resolve(&self, _: &LeaseClaims, input: Value, configuration: Value) -> Result<Value> {
        if configuration != json!({}) {
            return Err(invalid());
        }
        self.resolve_ref(serde_json::from_value(input).map_err(|_| invalid())?)
            .await
    }
    async fn archive(
        &self,
        _: &LeaseClaims,
        input: Value,
        configuration: Value,
    ) -> Result<Vec<u8>> {
        if configuration != json!({}) {
            return Err(invalid());
        }
        self.source(serde_json::from_value(input).map_err(|_| invalid())?)
            .await
    }
}
mod archive;
