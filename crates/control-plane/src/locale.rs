use crate::http::{ServiceState, reject};
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;
use std::sync::Arc;
pub fn preference(headers: &HeaderMap) -> &'static str {
    let mut found = None;
    for header in headers.get_all(header::COOKIE) {
        let Ok(raw) = header.to_str() else {
            return "system";
        };
        for piece in raw.split(';') {
            if let Some(value) = piece.trim().strip_prefix("webdock_locale=") {
                if found.is_some() {
                    return "system";
                }
                found = Some(value);
            }
        }
    }
    match found {
        Some("en") => "en",
        Some("de") => "de",
        _ => "system",
    }
}
pub fn routes() -> Router<Arc<ServiceState>> {
    Router::new().route("/api/locale", post(save))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preference {
    preference: String,
}
async fn save(State(state): State<Arc<ServiceState>>, headers: HeaderMap, body: Bytes) -> Response {
    let result = async {
        if body.len() > 4096
            || headers
                .get(header::CONTENT_TYPE)
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.split(';').next())
                .map(str::trim)
                != Some("application/json")
        {
            return Err(reject(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "Invalid language preference.",
            ));
        }
        let body: Preference = serde_json::from_slice(&body).map_err(|_| {
            reject(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "Invalid language preference.",
            )
        })?;
        if !["en", "de", "system"].contains(&body.preference.as_str()) {
            return Err(reject(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "Invalid language preference.",
            ));
        }
        let origin = crate::browser::request_origin(&state, &headers).ok_or_else(|| {
            reject(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Request unavailable.",
            )
        })?;
        let secure = if origin.starts_with("https://") {
            "; Secure"
        } else {
            ""
        };
        let mut response = StatusCode::NO_CONTENT.into_response();
        response.headers_mut().insert(
            header::SET_COOKIE,
            format!(
                "webdock_locale={}; Path=/; Max-Age=31536000; HttpOnly; SameSite=Lax{secure}",
                body.preference
            )
            .parse()
            .expect("fixed language value"),
        );
        Ok(response)
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_and_unknown_preferences_use_system() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, "webdock_locale=de".parse().unwrap());
        assert_eq!(preference(&h), "de");
        h.append(header::COOKIE, "webdock_locale=en".parse().unwrap());
        assert_eq!(preference(&h), "system");
    }
}
