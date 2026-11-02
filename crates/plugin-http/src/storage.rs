//! Dedicated binary lane. Does not relax HttpBinding's fixed path/query policy.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use url::Url;
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageBinding {
    pub origin: String,
    pub addresses: Vec<IpAddr>,
    pub ca: Option<String>,
    pub path_prefix: String,
}
pub struct StorageTarget {
    url: Url,
    headers: BTreeMap<String, String>,
    binding: StorageBinding,
    method: String,
    insecure_test: bool,
}
pub fn bind_storage(
    method: &str,
    url: &str,
    headers: BTreeMap<String, String>,
    binding: StorageBinding,
) -> Result<StorageTarget> {
    bind(method, url, headers, binding, false)
}
/// Explicit test constructor; HTTP is allowed only to literal, pinned loopback addresses.
pub fn bind_storage_loopback_test(
    method: &str,
    url: &str,
    headers: BTreeMap<String, String>,
    binding: StorageBinding,
) -> Result<StorageTarget> {
    bind(method, url, headers, binding, true)
}
fn bind(
    method: &str,
    url: &str,
    headers: BTreeMap<String, String>,
    binding: StorageBinding,
    insecure_test: bool,
) -> Result<StorageTarget> {
    let deny = || Error::Denied;
    let origin = Url::parse(&binding.origin).map_err(|_| deny())?;
    let target = Url::parse(url).map_err(|_| deny())?;
    if !["GET", "PUT"].contains(&method)
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || target.origin() != origin.origin()
        || !target.username().is_empty()
        || target.password().is_some()
        || target.fragment().is_some()
        || binding.addresses.is_empty()
        || binding.addresses.len() > 16
        || !binding.path_prefix.starts_with('/')
        || !binding.path_prefix.ends_with('/')
        || binding.path_prefix.contains(['%', '?', '#', '\\'])
        || binding
            .path_prefix
            .split('/')
            .any(|p| p == "." || p == "..")
        || !target.path().starts_with(&binding.path_prefix)
        || target.path().len() <= binding.path_prefix.len()
        || !target
            .path()
            .trim_start_matches('/')
            .split_once('/')
            .is_some_and(|(bucket, key)| !bucket.is_empty() && !key.is_empty())
        || !object_query(method, &target)
        || headers.iter().any(|(k, v)| {
            ![
                "content-type",
                "if-match",
                "if-none-match",
                "x-amz-checksum-sha256",
                "x-amz-content-sha256",
            ]
            .contains(&k.as_str())
                || k == "if-none-match" && (method != "PUT" || v != "*")
                || k == "if-match" && (method != "GET" || !object_etag(v))
                || v.len() > 2048
                || reqwest::header::HeaderValue::from_str(v).is_err()
        })
    {
        return Err(deny());
    }
    let hostname = origin.host_str().ok_or_else(deny)?;
    let ip = hostname.trim_matches(['[', ']']).parse::<IpAddr>().ok();
    if ip.is_some_and(|ip| !binding.addresses.contains(&ip)) {
        return Err(deny());
    }
    if origin.scheme() != "https"
        && !(insecure_test
            && origin.scheme() == "http"
            && ip.is_some_and(|ip| ip.is_loopback())
            && binding.addresses.iter().all(IpAddr::is_loopback))
    {
        return Err(deny());
    }
    Ok(StorageTarget {
        url: target,
        headers,
        binding,
        method: method.into(),
        insecure_test,
    })
}
// Only object reads/writes use this lane. Multipart has no Core reservation/receipt yet.
fn object_query(method: &str, target: &Url) -> bool {
    let Some(query) = target.query() else {
        return true;
    };
    let mut seen = std::collections::BTreeSet::new();
    for (raw, (key, value)) in query.split('&').zip(target.query_pairs()) {
        let name = raw.split('=').next().unwrap_or_default();
        if name != key || !seen.insert(key.to_string()) || value.is_empty() {
            return false;
        }
        let permitted = match key.as_ref() {
            "X-Amz-Algorithm"
            | "X-Amz-Credential"
            | "X-Amz-Date"
            | "X-Amz-Expires"
            | "X-Amz-SignedHeaders"
            | "X-Amz-Signature"
            | "X-Amz-Security-Token"
            | "X-Amz-Content-Sha256" => true,
            "x-amz-checksum-sha256" | "x-amz-sdk-checksum-algorithm" => method == "PUT",
            "versionId" => method == "GET",
            "x-id" => {
                value
                    == if method == "PUT" {
                        "PutObject"
                    } else {
                        "GetObject"
                    }
            }
            _ => false,
        };
        if !permitted {
            return false;
        }
    }
    true
}
fn object_etag(value: &str) -> bool {
    value.len() >= 3
        && value.len() <= 130
        && value.starts_with('"')
        && value.ends_with('"')
        && value[1..value.len() - 1]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
/// Exactly one attempt. The caller must poll its live authority while awaiting and reading.
/// Only status, content length and a sanitized ETag are consumed by the binary kernel.
pub async fn send_storage(
    target: StorageTarget,
    body: Option<reqwest::Body>,
    exact_bytes: u64,
) -> Result<reqwest::Response> {
    let deny = || Error::Denied;
    if (target.method == "PUT") != body.is_some() {
        return Err(deny());
    }
    let host = target.url.host_str().ok_or_else(deny)?;
    let port = target.url.port_or_known_default().ok_or_else(deny)?;
    let addresses = target
        .binding
        .addresses
        .iter()
        .map(|ip| SocketAddr::new(*ip, port))
        .collect::<Vec<_>>();
    let mut client = reqwest::Client::builder()
        .https_only(!target.insecure_test)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .pool_max_idle_per_host(0)
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(300))
        .resolve_to_addrs(host, &addresses);
    if let Some(ca) = &target.binding.ca {
        client = client.add_root_certificate(
            reqwest::Certificate::from_pem(ca.as_bytes()).map_err(|_| deny())?,
        );
    }
    let client = client.build().map_err(|_| Error::Transport)?;
    let mut request = client.request(
        reqwest::Method::from_bytes(target.method.as_bytes()).map_err(|_| deny())?,
        target.url,
    );
    for (k, v) in &target.headers {
        let mut v = reqwest::header::HeaderValue::from_str(v).map_err(|_| deny())?;
        v.set_sensitive(true);
        request = request.header(k, v);
    }
    request = request.header(reqwest::header::ACCEPT_ENCODING, "identity");
    if let Some(body) = body {
        request = request
            .header(reqwest::header::CONTENT_LENGTH, exact_bytes)
            .body(body);
    }
    let response = request.send().await.map_err(|_| Error::Transport)?;
    if response.status() != reqwest::StatusCode::OK
        && !(target.method == "PUT" && response.status() == reqwest::StatusCode::NO_CONTENT)
        || response
            .headers()
            .contains_key(reqwest::header::CONTENT_ENCODING)
        || target.method == "GET" && response.content_length() != Some(exact_bytes)
        || target.method == "PUT" && response.content_length().is_some_and(|n| n > 8192)
    {
        return Err(Error::Transport);
    }
    Ok(response)
}
