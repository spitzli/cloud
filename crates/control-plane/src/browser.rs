use crate::{
    auth,
    http::{ApiFailure, ServiceState, reject},
};
use axum::{
    Json, Router,
    extract::{RawQuery, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use std::{collections::HashMap, sync::Arc};
use webdock_browser_session::{BrowserRedirect, BrowserSessions, Error};

pub fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .route("/api/sso/login", get(login))
        .route("/api/sso/callback", get(callback))
        .route("/api/sso/logout", post(logout))
        .route("/api/users/logout", post(logout))
        .route("/api/users/refresh-token", post(refresh))
        .route(
            "/api/auth/browser/logout",
            get(logout_details).post(logout_confirm),
        )
}
pub fn protocol_path(path: &str) -> bool {
    path.starts_with("/api/sso/")
        || ["/api/users/logout", "/api/users/refresh-token"].contains(&path)
}
pub fn request_origin<'a>(state: &'a ServiceState, headers: &HeaderMap) -> Option<&'a str> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    state.config.allowed_origins().into_iter().find(|v| {
        url::Url::parse(v)
            .ok()
            .is_some_and(|u| &u[url::Position::BeforeHost..url::Position::AfterPort] == host)
    })
}
pub fn failure(error: Error) -> ApiFailure {
    match error {
        Error::Unauthorized => reject(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "Sign in to continue.",
        ),
        Error::InvalidInput => reject(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "The sign-in request is invalid.",
        ),
        _ => reject(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Sign-in is unavailable. Try again.",
        ),
    }
}
fn service(state: &ServiceState) -> Result<&BrowserSessions, ApiFailure> {
    state
        .browser
        .as_deref()
        .ok_or_else(|| failure(Error::Configuration))
}
fn query(raw: Option<String>, allowed: &[&str]) -> Result<HashMap<String, String>, ApiFailure> {
    let raw = raw.unwrap_or_default();
    if raw.len() > 8192 {
        return Err(failure(Error::InvalidInput));
    }
    let mut fields = HashMap::new();
    for (k, v) in url::form_urlencoded::parse(raw.as_bytes()) {
        if !allowed.contains(&k.as_ref()) || fields.insert(k.into_owned(), v.into_owned()).is_some()
        {
            return Err(failure(Error::InvalidInput));
        }
    }
    Ok(fields)
}
fn response(value: BrowserRedirect) -> Response {
    let mut response = Redirect::to(&value.location).into_response();
    for cookie in value.set_cookies {
        if let Ok(cookie) = cookie.parse() {
            response.headers_mut().append(header::SET_COOKIE, cookie);
        }
    }
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    response
}
async fn login(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let result = async {
        if state.auth_limiter.check().is_err() {
            return Err(reject(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "Try again shortly.",
            ));
        }
        let fields = query(raw, &["returnTo"])?;
        service(&state)?
            .begin(
                request_origin(&state, &headers).ok_or_else(|| failure(Error::InvalidInput))?,
                fields
                    .get("returnTo")
                    .map(String::as_str)
                    .unwrap_or("/projects"),
            )
            .await
            .map_err(failure)
    }
    .await;
    match result {
        Ok(value) => response(value),
        Err(error) => error.into_response(),
    }
}
async fn callback(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let result = async {
        let fields = query(raw, &["code", "state", "iss", "error", "error_description"])?;
        if fields.contains_key("error") {
            return Err(failure(Error::Unauthorized));
        }
        if fields.get("iss").map(String::as_str) != Some(service(&state)?.config.issuer.as_str()) {
            return Err(failure(Error::Unauthorized));
        }

        service(&state)?
            .callback(
                request_origin(&state, &headers).ok_or_else(|| failure(Error::InvalidInput))?,
                &auth::cookie(&headers)?,
                fields
                    .get("code")
                    .ok_or_else(|| failure(Error::InvalidInput))?,
                fields
                    .get("state")
                    .ok_or_else(|| failure(Error::InvalidInput))?,
            )
            .await
            .map_err(failure)
    }
    .await;
    match result {
        Ok(value) => response(value),
        Err(_) => Redirect::to("/login?error=sign-in").into_response(),
    }
}
async fn logout(State(state): State<Arc<ServiceState>>, headers: HeaderMap) -> Response {
    let result = async {
        service(&state)?
            .logout(
                request_origin(&state, &headers).ok_or_else(|| failure(Error::InvalidInput))?,
                &auth::cookie(&headers)?,
            )
            .await
            .map_err(failure)
    }
    .await;
    match result {
        Ok(value)
            if headers
                .get(header::ACCEPT)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.contains("application/json")) =>
        {
            let mut result = Json(serde_json::json!({"redirect":value.location})).into_response();
            for cookie in value.set_cookies {
                if let Ok(cookie) = cookie.parse() {
                    result.headers_mut().append(header::SET_COOKIE, cookie);
                }
            }
            result
        }
        Ok(value) => response(value),
        Err(error) => error.into_response(),
    }
}
async fn refresh(State(state): State<Arc<ServiceState>>, headers: HeaderMap) -> Response {
    match auth::resolve(&state,&headers).await{Ok(Some(actor))=>Json(serde_json::json!({"user":auth::public_user(&actor),"exp":actor.expires_at.timestamp()})).into_response(),Ok(None)=>failure(Error::Unauthorized).into_response(),Err(error)=>error.into_response()}
}

async fn logout_details(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let result = async {
        let q = query(raw, &["request"])?;
        service(&state)?
            .logout_details(
                &auth::cookie(&headers)?,
                q.get("request")
                    .ok_or_else(|| failure(Error::InvalidInput))?,
            )
            .await
            .map_err(failure)
    }
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => error.into_response(),
    }
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LogoutConfirm {
    request: String,
    confirmed: bool,
}
async fn logout_confirm(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Json(input): Json<LogoutConfirm>,
) -> Response {
    let result = async {
        service(&state)?
            .finish_logout(&auth::cookie(&headers)?, &input.request, input.confirmed)
            .await
            .map_err(failure)
    }
    .await;
    match result {
        Ok(value) => {
            let mut response = Json(serde_json::json!({"redirect":value.location})).into_response();
            for cookie in value.set_cookies {
                if let Ok(cookie) = cookie.parse() {
                    response.headers_mut().append(header::SET_COOKIE, cookie);
                }
            }
            response
        }
        Err(error) => error.into_response(),
    }
}
