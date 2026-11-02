//! Fixed named HTTPS policy shared by independent providers and the Core host broker.
pub mod storage;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use url::Url;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Named HTTP binding denied")]
    Denied,
    #[error("Provider request exceeds limit")]
    Limit,
    #[error("Provider transport unavailable")]
    Transport,
}
pub type Result<T> = std::result::Result<T, Error>;
fn denied() -> Error {
    Error::Denied
}
fn transport_error() -> Error {
    Error::Transport
}
pub const MAIL_REQUEST_BYTES: usize = 25 * 1024 * 1024;
pub const MAIL_RESPONSE_BYTES: usize = 64 * 1024;
const JSON_BYTES: usize = 1024 * 1024;
#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpBinding {
    pub origin: String,
    pub addresses: Vec<IpAddr>,
    pub headers: BTreeMap<String, String>,
    pub rules: Vec<HttpRule>,
    pub ca: Option<String>,
    #[serde(default, rename = "websiteBindingID")]
    pub website_binding_id: Option<String>,
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HttpRule {
    pub method: String,
    pub path: String,
    pub query: Vec<String>,
}
pub fn validate_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.encode_utf16().count() > 2048
        || !path.starts_with('/')
        || path.starts_with("//")
        || path
            .chars()
            .any(|c| c <= ' ' || matches!(c, '\\' | '%' | '#'))
        || path
            .split('?')
            .next()
            .unwrap()
            .split('/')
            .any(|p| p == "." || p == "..")
    {
        return Err(denied());
    }
    Ok(())
}

/// Constructed only by bind_target, so transport callers cannot substitute a URL after validation.
pub struct BoundTarget {
    url: Url,
    binding: HttpBinding,
    method: String,
}
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}
pub fn bind_target(method: &str, path: &str, binding: HttpBinding) -> Result<BoundTarget> {
    validate_path(path)?;
    if !["GET", "POST", "PUT", "PATCH", "DELETE"].contains(&method) {
        return Err(denied());
    }
    let origin = Url::parse(&binding.origin).map_err(|_| denied())?;
    if origin.scheme() != "https"
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || binding.addresses.is_empty()
        || binding.addresses.len() > 16
        || binding.website_binding_id.is_some()
    {
        return Err(denied());
    }
    let host = origin.host_str().ok_or_else(denied)?;
    if let Ok(address) = host.trim_matches(['[', ']']).parse::<IpAddr>()
        && !binding.addresses.contains(&address)
    {
        return Err(denied());
    }
    let url = origin.join(path).map_err(|_| denied())?;
    if url.origin() != origin.origin() {
        return Err(denied());
    }
    let rule = binding
        .rules
        .iter()
        .find(|rule| rule.method == method && rule.path == url.path())
        .ok_or_else(denied)?;
    let mut seen = HashSet::new();
    for (key, _) in url.query_pairs() {
        if !rule.query.iter().any(|allowed| allowed == &key) || !seen.insert(key.into_owned()) {
            return Err(denied());
        }
    }
    for key in binding.headers.keys() {
        if [
            "host",
            "content-length",
            "transfer-encoding",
            "connection",
            "proxy-authorization",
            "proxy-connection",
            "upgrade",
        ]
        .contains(&key.to_ascii_lowercase().as_str())
        {
            return Err(denied());
        }
    }
    Ok(BoundTarget {
        url,
        binding,
        method: method.to_owned(),
    })
}
/// Ordinary host requests retain their original 1 MiB request/response ceiling.
pub async fn send_json(target: BoundTarget, body: Option<Vec<u8>>) -> Result<HttpResponse> {
    send(
        target,
        "application/json",
        body,
        JSON_BYTES,
        JSON_BYTES,
        Duration::from_secs(10),
    )
    .await
}
/// Separate durable Mail lane. Its caller must hold the one-use domain attempt authority.
/// This function performs exactly one HTTP attempt and never retries or follows redirects.
pub async fn send_mail(
    target: BoundTarget,
    content_type: &str,
    body: Vec<u8>,
) -> Result<HttpResponse> {
    if !["message/rfc822", "application/json"].contains(&content_type) {
        return Err(denied());
    }
    send(
        target,
        content_type,
        Some(body),
        MAIL_REQUEST_BYTES,
        MAIL_RESPONSE_BYTES,
        Duration::from_secs(30),
    )
    .await
}
async fn send(
    input: BoundTarget,
    content_type: &str,
    body: Option<Vec<u8>>,
    request_limit: usize,
    response_limit: usize,
    deadline: Duration,
) -> Result<HttpResponse> {
    if body.as_ref().is_some_and(|b| b.len() > request_limit) {
        return Err(Error::Limit);
    }
    if input.method == "GET" && body.is_some() {
        return Err(denied());
    }
    let hostname = input.url.host_str().ok_or_else(denied)?;
    let port = input.url.port_or_known_default().ok_or_else(denied)?;
    let addresses = input
        .binding
        .addresses
        .iter()
        .map(|ip| SocketAddr::new(*ip, port))
        .collect::<Vec<_>>();
    let mut client = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .pool_max_idle_per_host(0)
        .connect_timeout(Duration::from_secs(3))
        .timeout(deadline)
        .resolve_to_addrs(hostname, &addresses);
    if let Some(ca) = &input.binding.ca {
        client = client.add_root_certificate(
            reqwest::Certificate::from_pem(ca.as_bytes()).map_err(|_| denied())?,
        );
    }
    let client = client.build().map_err(|_| transport_error())?;
    let mut request = client.request(
        reqwest::Method::from_bytes(input.method.as_bytes()).map_err(|_| denied())?,
        input.url,
    );
    for (key, value) in &input.binding.headers {
        let name = reqwest::header::HeaderName::from_bytes(key.as_bytes()).map_err(|_| denied())?;
        let mut value = reqwest::header::HeaderValue::from_str(value).map_err(|_| denied())?;
        value.set_sensitive(true);
        request = request.header(name, value);
    }
    request = request.header(reqwest::header::ACCEPT, "application/json");
    if let Some(body) = body {
        request = request
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(body);
    }
    let response = request.send().await.map_err(|_| transport_error())?;
    let status = response.status().as_u16();
    if response
        .content_length()
        .is_some_and(|length| length > response_limit as u64)
    {
        return Err(transport_error());
    }
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| transport_error())?;
        if body.len().saturating_add(chunk.len()) > response_limit {
            return Err(transport_error());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(HttpResponse { status, body })
}
