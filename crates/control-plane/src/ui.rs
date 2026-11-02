//! Optional immutable first-party UI artifacts. API paths never use SPA fallback.
use axum::{
    body::Body,
    extract::Request,
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

pub struct UiAssets {
    dist: PathBuf,
    csp: HeaderValue,
    connections: Vec<String>,
}
#[derive(Clone)]
pub(crate) struct ImmutableUiAsset;
impl UiAssets {
    pub fn new(
        dist: impl AsRef<Path>,
        frames: &[String],
        connections: &[String],
        production: bool,
    ) -> Result<Self, &'static str> {
        let dist = dist
            .as_ref()
            .canonicalize()
            .map_err(|_| "UI artifact directory unavailable")?;
        if !dist.is_dir()
            || !dist.join("index.html").is_file()
            || !dist
                .join("index.html")
                .canonicalize()
                .map_err(|_| "UI index unavailable")?
                .starts_with(&dist)
        {
            return Err("Built UI index.html required");
        }
        let sources = |origins: &[String]| -> Result<Vec<String>, &'static str> {
            if origins.len() > 32 {
                return Err("UI origin allowlist is too large");
            }
            let mut allowed = vec![];
            for raw in origins {
                let u = url::Url::parse(raw).map_err(|_| "Invalid UI origin allowlist")?;
                let local = !production
                    && u.scheme() == "http"
                    && [Some("localhost"), Some("127.0.0.1"), Some("[::1]")]
                        .contains(&u.host_str());
                if (!local && u.scheme() != "https")
                    || u.path() != "/"
                    || u.query().is_some()
                    || u.fragment().is_some()
                    || !u.username().is_empty()
                    || u.password().is_some()
                    || u.host_str().is_none()
                {
                    return Err("UI allowlists require exact HTTPS origins");
                }
                allowed.push(u.origin().ascii_serialization());
            }
            allowed.sort();
            allowed.dedup();
            Ok(allowed)
        };
        let frames = sources(frames)?.join(" ");
        let allowed = sources(connections)?;
        let connections = allowed.join(" ");
        let sockets = allowed
            .iter()
            .map(|origin| {
                origin
                    .replacen("https://", "wss://", 1)
                    .replacen("http://", "ws://", 1)
            })
            .collect::<Vec<_>>()
            .join(" ");
        let csp = format!(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; worker-src 'self' blob:; connect-src 'self' {connections} {sockets}; frame-src {}; frame-ancestors 'none'; base-uri 'none'; object-src 'none'; form-action 'self'",
            if frames.is_empty() { "'none'" } else { &frames }
        );
        Ok(Self {
            dist,
            csp: csp.parse().map_err(|_| "Invalid UI content policy")?,
            connections: allowed,
        })
    }
    /// A gateway the Core advertises to this UI must be allowed by its `connect-src`.
    pub fn require_connect(&self, origin: &str) -> Result<(), &'static str> {
        let origin = url::Url::parse(origin)
            .map_err(|_| "Invalid gateway origin")?
            .origin()
            .ascii_serialization();
        if self.connections.contains(&origin) {
            Ok(())
        } else {
            Err("Add each browser gateway origin to WEBDOCK_UI_CONNECT_ORIGINS")
        }
    }
    pub fn from_map(
        env: &HashMap<String, String>,
        production: bool,
    ) -> Result<Option<Self>, &'static str> {
        let Some(dist) = env.get("WEBDOCK_UI_DIST") else {
            return Ok(None);
        };
        let list = |key: &str| -> Result<Vec<String>, &'static str> {
            env.get(key)
                .map(|v| {
                    serde_json::from_str(v)
                        .map_err(|_| "UI allowlists must be JSON arrays of exact origins")
                })
                .transpose()
                .map(Option::unwrap_or_default)
        };
        Self::new(
            dist,
            &list("WEBDOCK_UI_FRAME_ORIGINS")?,
            &list("WEBDOCK_UI_CONNECT_ORIGINS")?,
            production,
        )
        .map(Some)
    }
    pub async fn serve(&self, request: Request) -> Response {
        if !matches!(*request.method(), Method::GET | Method::HEAD) {
            return missing();
        }
        let Ok(decoded) = percent_encoding::percent_decode_str(request.uri().path()).decode_utf8()
        else {
            return missing();
        };
        if !ui_path(&decoded)
            || decoded.contains(['\\', '%'])
            || decoded.contains("//")
            || decoded.chars().any(|c| c <= ' ')
        {
            return missing();
        }
        let relative = Path::new(decoded.trim_start_matches('/'));
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
            || relative
                .components()
                .any(|part| part.as_os_str().to_string_lossy().starts_with('.'))
        {
            return missing();
        }
        let candidate = self.dist.join(relative);
        if let Ok(path) = candidate.canonicalize()
            && !path.starts_with(&self.dist)
        {
            return missing();
        }
        let immutable = decoded.starts_with("/assets/")
            && relative
                .file_name()
                .and_then(|v| v.to_str())
                .is_some_and(|name| {
                    name.rsplit_once('-').is_some_and(|(_, suffix)| {
                        suffix.split('.').next().is_some_and(|hash| {
                            hash.len() >= 8
                                && hash.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                        })
                    })
                });
        let is_asset = decoded.starts_with("/assets/") || relative.extension().is_some();
        let result = if is_asset {
            ServeDir::new(&self.dist)
                .append_index_html_on_directories(false)
                .oneshot(request)
                .await
                .map(|r| r.map(Body::new))
        } else {
            ServeDir::new(&self.dist)
                .append_index_html_on_directories(false)
                .fallback(ServeFile::new(self.dist.join("index.html")))
                .oneshot(request)
                .await
                .map(|r| r.map(Body::new))
        };
        let Ok(mut response) = result;
        let html = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/html"));
        response.headers_mut().insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        );
        if html {
            response
                .headers_mut()
                .insert(header::CONTENT_SECURITY_POLICY, self.csp.clone());
            response
                .headers_mut()
                .insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        } else if immutable && response.status().is_success() {
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            );
            response.extensions_mut().insert(ImmutableUiAsset);
        }
        response
    }
}
pub fn ui_path(path: &str) -> bool {
    ![
        "/api",
        "/v1",
        "/.well-known",
        "/healthz",
        "/readyz",
        "/console-config.json",
    ]
    .iter()
    .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
}
fn missing() -> Response {
    StatusCode::NOT_FOUND.into_response()
}
