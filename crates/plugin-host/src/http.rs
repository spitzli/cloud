use crate::{HostService, InvocationHandle, invalid, unavailable, unsupported};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use url::Url;
use webdock_plugin_protocol::{ErrorCode, PluginError, Result};
const LIMIT: usize = 1024 * 1024;
pub use webdock_plugin_http::{HttpBinding, HttpRule};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HttpResult {
    pub status: u16,
    pub body: Value,
}
pub struct BoundRequest {
    pub url: Url,
    pub binding: HttpBinding,
    pub method: String,
    pub body: Option<String>,
}
#[async_trait]
pub trait HttpTransport: Send + Sync {
    async fn send(&self, input: BoundRequest) -> Result<HttpResult>;
}
pub struct PinnedTransport;
fn denied() -> PluginError {
    PluginError::denied()
}
fn provider_error() -> PluginError {
    let mut error = unavailable();
    error.retryable = true;
    error
}
pub fn validate_path(path: &str) -> Result<()> {
    webdock_plugin_http::validate_path(path).map_err(|_| denied())
}
pub fn validate_request(input: &Value) -> Result<()> {
    let raw = input
        .as_object()
        .ok_or_else(|| invalid("Invalid HTTP request"))?;
    if raw
        .keys()
        .any(|key| !["binding", "method", "path", "body"].contains(&key.as_str()))
    {
        return Err(invalid("Invalid HTTP request"));
    }
    let name = input["binding"]
        .as_str()
        .ok_or_else(|| invalid("Invalid HTTP binding"))?;
    if name.is_empty()
        || name.len() > 64
        || !name.as_bytes()[0].is_ascii_lowercase()
        || !name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-".contains(&c))
    {
        return Err(invalid("Invalid HTTP binding"));
    }
    let method = input["method"]
        .as_str()
        .filter(|m| ["GET", "POST", "PUT", "PATCH", "DELETE"].contains(m))
        .ok_or_else(|| invalid("Invalid HTTP method"))?;
    let path = input["path"]
        .as_str()
        .ok_or_else(|| invalid("Invalid HTTP path"))?;
    if path.is_empty() || path.encode_utf16().count() > 2048 {
        return Err(invalid("Invalid HTTP path"));
    }
    validate_path(path)?;
    if method == "GET" && raw.contains_key("body") {
        return Err(denied());
    }
    Ok(())
}
pub fn bind_request(input: &Value, binding: HttpBinding) -> Result<BoundRequest> {
    validate_request(input)?;
    let raw = input.as_object().unwrap();
    let method = input["method"].as_str().unwrap();
    let path = input["path"].as_str().unwrap();
    webdock_plugin_http::bind_target(method, path, binding.clone()).map_err(|_| denied())?;
    let url = Url::parse(&binding.origin)
        .map_err(|_| denied())?
        .join(path)
        .map_err(|_| denied())?;
    let body = raw
        .get("body")
        .map(|value| crate::data::javascript_json(value, false))
        .transpose()?;
    if body.as_ref().is_some_and(|body| body.len() > LIMIT) {
        return Err(invalid("Provider request exceeds limit"));
    }
    Ok(BoundRequest {
        url,
        binding,
        method: method.to_owned(),
        body,
    })
}
#[async_trait]
impl HttpTransport for PinnedTransport {
    async fn send(&self, input: BoundRequest) -> Result<HttpResult> {
        let path = match input.url.query() {
            Some(query) => format!("{}?{query}", input.url.path()),
            None => input.url.path().to_owned(),
        };
        let target = webdock_plugin_http::bind_target(&input.method, &path, input.binding)
            .map_err(|_| denied())?;
        let response = webdock_plugin_http::send_json(target, input.body.map(String::into_bytes))
            .await
            .map_err(|e| match e {
                webdock_plugin_http::Error::Denied => denied(),
                webdock_plugin_http::Error::Limit => invalid("Provider request exceeds limit"),
                webdock_plugin_http::Error::Transport => provider_error(),
            })?;
        let body = if response.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&response.body).map_err(|_| provider_error())?
        };
        Ok(HttpResult {
            status: response.status,
            body,
        })
    }
}
impl HostService {
    async fn http_binding(&self, handle: &InvocationHandle, input: &Value) -> Result<HttpBinding> {
        let capability = if input["method"] == "GET" {
            "plugin.http.read"
        } else {
            "plugin.http.request"
        };
        let context = self.authorized(handle, Some(capability)).await?;
        let name = input["binding"]
            .as_str()
            .ok_or_else(|| invalid("Invalid HTTP binding"))?;
        if !context.manifest.as_json()["httpBindings"]
            .as_array()
            .is_some_and(|names| names.iter().any(|v| v == name))
        {
            return Err(denied());
        }
        let binding = context
            .binding
            .projects
            .get(&context.scope.project_id)
            .and_then(|bindings| bindings.get(name))
            .cloned()
            .ok_or_else(|| unsupported("HTTP binding is not configured for this project"))?;
        bind_request(input, binding.clone())?;
        self.finish(context, handle, Some(capability)).await?;
        Ok(binding)
    }
    pub async fn http_request(
        &self,
        handle: &InvocationHandle,
        input: Value,
    ) -> Result<HttpResult> {
        self.request_http(handle, input, &PinnedTransport).await
    }
    #[cfg(feature = "test-transport")]
    pub async fn http_request_with_test_transport(
        &self,
        handle: &InvocationHandle,
        input: Value,
        transport: &dyn HttpTransport,
    ) -> Result<HttpResult> {
        self.request_http(handle, input, transport).await
    }
    async fn request_http(
        &self,
        handle: &InvocationHandle,
        input: Value,
        transport: &dyn HttpTransport,
    ) -> Result<HttpResult> {
        if input["binding"] == "neon" && self.named_http.is_some() {
            return self.request_named_http(handle, input).await;
        }
        validate_request(&input)?;
        let binding = self.http_binding(handle, &input).await?;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            transport.send(bind_request(&input, binding.clone())?),
        )
        .await
        .map_err(|_| {
            PluginError::new(ErrorCode::DeadlineExceeded, "Provider deadline exceeded")
        })??;
        let fresh = self.http_binding(handle, &input).await?;
        if fresh != binding {
            return Err(denied());
        }
        if (300..400).contains(&result.status) || [401, 403].contains(&result.status) {
            return Err(denied());
        }
        if result.status >= 500 {
            return Err(provider_error());
        }
        if serde_json::to_vec(&result.body)
            .map_err(|_| provider_error())?
            .len()
            > LIMIT
        {
            return Err(provider_error());
        }
        Ok(result)
    }
}

/// Contains only Core-admitted authority. It cannot be deserialized or built by a runtime.
pub struct HttpAdapterContext<'a> {
    pub(crate) host: &'a HostService,
    pub(crate) handle: &'a InvocationHandle,
    pub(crate) scope: crate::InvocationScope,
    pub(crate) invocation_id: String,
    pub(crate) action: String,
    pub(crate) write: bool,
    pub(crate) request_hash: String,
    pub(crate) resource_id: Option<String>,
    pub(crate) principal: webdock_identity::Principal,
    pub(crate) authority: webdock_browser_session::StoredAuthority,
}
impl HttpAdapterContext<'_> {
    pub async fn revalidate(&self) -> Result<()> {
        self.host.revalidate(self.handle).await
    }
    pub fn scope(&self) -> &crate::InvocationScope {
        &self.scope
    }
    pub fn invocation_id(&self) -> &str {
        &self.invocation_id
    }
    pub fn action(&self) -> &str {
        &self.action
    }
    pub fn write(&self) -> bool {
        self.write
    }
    pub fn requires_connection_management(&self) -> bool {
        crate::authority::neon_management(self.write, &self.action)
    }
    pub fn request_hash(&self) -> &str {
        &self.request_hash
    }
    pub fn resource_id(&self) -> Option<&str> {
        self.resource_id.as_deref()
    }
    pub fn principal(&self) -> &webdock_identity::Principal {
        &self.principal
    }
    pub fn authority(&self) -> &webdock_browser_session::StoredAuthority {
        &self.authority
    }
    pub fn matches_input(&self, input: &Value) -> Result<bool> {
        Ok(
            crate::authority::digest(crate::data::javascript_json(input, false)?)
                == self.request_hash,
        )
    }
}
#[async_trait]
pub trait NamedHttpAdapter: Send + Sync {
    async fn request(&self, context: &HttpAdapterContext<'_>, input: Value) -> Result<HttpResult>;
}
/// Structured query is deliberately a separate validator: legacy static bindings are unchanged.
pub fn validate_named_request(input: &Value) -> Result<()> {
    let raw = input
        .as_object()
        .ok_or_else(|| invalid("Invalid HTTP request"))?;
    if input["binding"] != "neon"
        || raw
            .keys()
            .any(|key| !["binding", "method", "path", "query", "body"].contains(&key.as_str()))
        || !matches!(input["method"].as_str(), Some("GET" | "POST"))
    {
        return Err(denied());
    }
    let path = input["path"].as_str().ok_or_else(denied)?;
    validate_path(path)?;
    if path.contains('?') {
        return Err(denied());
    }
    if let Some(query) = raw.get("query") {
        let query = query.as_object().ok_or_else(denied)?;
        if query.len() > 2
            || query.iter().any(|(k, v)| {
                !["cursor", "limit"].contains(&k.as_str())
                    || v.as_str().is_none_or(|v| {
                        v.is_empty()
                            || v.len() > 1024
                            || v.chars().any(char::is_control)
                            || (k == "limit"
                                && !v.parse::<u16>().is_ok_and(|n| (1..=100).contains(&n)))
                    })
            })
        {
            return Err(denied());
        }
    }
    if input["method"] == "GET" && raw.contains_key("body") {
        return Err(denied());
    }
    if serde_json::to_vec(input).map_err(|_| denied())?.len() > 32768 {
        return Err(denied());
    }
    Ok(())
}
impl HostService {
    async fn request_named_http(
        &self,
        handle: &InvocationHandle,
        input: Value,
    ) -> Result<HttpResult> {
        validate_named_request(&input)?;
        let capability = if input["method"] == "GET" {
            "plugin.http.read"
        } else {
            "plugin.http.request"
        };
        let mut authorized = self.authorized(handle, Some(capability)).await?;
        if !authorized.manifest.as_json()["httpBindings"]
            .as_array()
            .is_some_and(|names| names.iter().any(|name| name == "neon"))
        {
            return Err(denied());
        }
        let context = authorized.http_adapter_context(self, handle)?;
        self.finish(authorized, handle, Some(capability)).await?;
        let result = tokio::time::timeout(
            Duration::from_secs(45),
            self.named_http
                .as_ref()
                .ok_or_else(denied)?
                .request(&context, input),
        )
        .await
        .map_err(|_| provider_error())??;
        let fresh = self.authorized(handle, Some(capability)).await?;
        self.finish(fresh, handle, Some(capability)).await?;
        if serde_json::to_vec(&result.body)
            .map_err(|_| provider_error())?
            .len()
            > LIMIT
        {
            return Err(provider_error());
        }
        Ok(result)
    }
}
