//! HTTP decoding and origin boundaries for the retained OAuth protocol.
use crate::http::ServiceState;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{RawQuery, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};
use webdock_oauth::{
    AuthorizationOutcome, AuthorizationRequest, ClientAuthMethod, ClientAuthentication,
    CodeExchange, Error, LogoutOutcome, LogoutRequest, OAuthService, RefreshExchange,
};

#[derive(Debug)]
struct Fields {
    values: BTreeMap<String, String>,
    resources: Vec<String>,
}
impl Fields {
    fn required(&self, key: &str) -> Result<String, Error> {
        self.values
            .get(key)
            .filter(|v| !v.is_empty())
            .cloned()
            .ok_or(Error::InvalidRequest)
    }
    fn optional(&self, key: &str) -> Option<String> {
        self.values.get(key).cloned()
    }
    fn only(&self, allowed: &[&str]) -> Result<(), Error> {
        if self
            .values
            .keys()
            .any(|key| !allowed.contains(&key.as_str()))
        {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }
}
fn decode(raw: &str) -> Result<String, Error> {
    let bytes = raw.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && (index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit())
        {
            return Err(Error::InvalidRequest);
        }
    }
    let replaced = raw.replace('+', " ");
    percent_encoding::percent_decode_str(&replaced)
        .decode_utf8()
        .map(|v| v.into_owned())
        .map_err(|_| Error::InvalidRequest)
}
fn fields(raw: &str, resources: bool) -> Result<Fields, Error> {
    if raw.len() > 16384 {
        return Err(Error::InvalidRequest);
    }
    let mut result = Fields {
        values: BTreeMap::new(),
        resources: vec![],
    };
    for pair in raw.split('&').filter(|v| !v.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode(key)?;
        let value = decode(value)?;
        if key == "resource" && resources {
            if result.resources.len() >= 8 || value.is_empty() {
                return Err(Error::InvalidRequest);
            }
            result.resources.push(value);
        } else if key.is_empty() || result.values.insert(key, value).is_some() {
            return Err(Error::InvalidRequest);
        }
        if result.values.len() > 32 {
            return Err(Error::InvalidRequest);
        }
    }
    Ok(result)
}
fn client(headers: &HeaderMap, fields: &Fields) -> Result<ClientAuthentication, Error> {
    if headers.get_all(header::AUTHORIZATION).iter().count() > 1 {
        return Err(Error::InvalidClient);
    }
    if let Some(raw) = headers.get(header::AUTHORIZATION) {
        if fields.values.contains_key("client_id") || fields.values.contains_key("client_secret") {
            return Err(Error::InvalidClient);
        }
        let raw = raw.to_str().map_err(|_| Error::InvalidClient)?;
        let (scheme, encoded) = raw.split_once(' ').ok_or(Error::InvalidClient)?;
        if !scheme.eq_ignore_ascii_case("basic") {
            return Err(Error::InvalidClient);
        }
        let bytes = STANDARD.decode(encoded).map_err(|_| Error::InvalidClient)?;
        let value = std::str::from_utf8(&bytes).map_err(|_| Error::InvalidClient)?;
        let (id, secret) = value.split_once(':').ok_or(Error::InvalidClient)?;
        let client_id = decode(id).map_err(|_| Error::InvalidClient)?;
        let client_secret = decode(secret).map_err(|_| Error::InvalidClient)?;
        if client_id.is_empty() || client_secret.is_empty() {
            return Err(Error::InvalidClient);
        }
        return Ok(ClientAuthentication {
            client_id,
            client_secret: Some(client_secret),
            method: ClientAuthMethod::ClientSecretBasic,
        });
    }
    let client_id = fields
        .required("client_id")
        .map_err(|_| Error::InvalidClient)?;
    let client_secret = fields.optional("client_secret");
    let method = if client_secret.is_some() {
        ClientAuthMethod::ClientSecretPost
    } else {
        ClientAuthMethod::None
    };
    Ok(ClientAuthentication {
        client_id,
        client_secret,
        method,
    })
}
fn service(state: &ServiceState) -> Result<&OAuthService, Error> {
    state.oauth.as_deref().ok_or(Error::Configuration)
}
fn oauth_error(error: Error) -> Response {
    let mut response = (
        StatusCode::from_u16(error.status()).expect("known status"),
        Json(json!({"error":error.code()})),
    )
        .into_response();
    if matches!(error, Error::InvalidClient) {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            "Basic realm=\"webdock\"".parse().unwrap(),
        );
    }
    response
        .headers_mut()
        .insert(header::PRAGMA, "no-cache".parse().unwrap());
    response
}
fn response(result: Result<Response, Error>) -> Response {
    result.unwrap_or_else(oauth_error)
}
fn form(headers: &HeaderMap, body: &[u8]) -> Result<Fields, Error> {
    if headers.get_all(header::CONTENT_TYPE).iter().count() != 1
        || headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim)
            != Some("application/x-www-form-urlencoded")
    {
        return Err(Error::InvalidRequest);
    }
    fields(
        std::str::from_utf8(body).map_err(|_| Error::InvalidRequest)?,
        false,
    )
}
fn bearer(headers: &HeaderMap) -> Result<&str, Error> {
    if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
        return Err(Error::InvalidGrant);
    }
    let raw = headers[header::AUTHORIZATION]
        .to_str()
        .map_err(|_| Error::InvalidGrant)?;
    let (scheme, token) = raw.split_once(' ').ok_or(Error::InvalidGrant)?;
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token.bytes().any(|v| v.is_ascii_whitespace())
    {
        return Err(Error::InvalidGrant);
    }
    Ok(token)
}
fn cookie(headers: &HeaderMap) -> Result<String, Error> {
    crate::auth::cookie(headers).map_err(|_| Error::InvalidRequest)
}
/// These endpoints authenticate protocol credentials rather than browser cookies. An Origin,
/// when supplied, is still checked by the shared boundary; absent Origin is valid for clients.
pub fn credential_endpoint(path: &str) -> bool {
    matches!(
        path,
        "/api/auth/oauth2/token"
            | "/api/auth/oauth2/introspect"
            | "/api/auth/oauth2/revoke"
            | "/api/auth/oauth2/userinfo"
    )
}
pub fn discovery_path(path: &str) -> bool {
    matches!(
        path,
        "/.well-known/openid-configuration" | "/.well-known/oauth-authorization-server/api/auth"
    )
}
pub fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route(
            "/.well-known/oauth-authorization-server/api/auth",
            get(discovery),
        )
        .route("/api/auth/.well-known/openid-configuration", get(discovery))
        .route("/api/auth/jwks", get(jwks))
        .route("/api/auth/oauth2/authorize", get(authorize))
        .route(
            "/api/auth/oauth2/consent",
            get(consent_details).post(consent),
        )
        .route("/api/auth/oauth2/token", post(token))
        .route("/api/auth/oauth2/introspect", post(introspect))
        .route("/api/auth/oauth2/revoke", post(revoke))
        .route("/api/auth/oauth2/userinfo", get(userinfo).post(userinfo))
        .route("/api/auth/oauth2/end-session", get(end_session))
}
async fn discovery(State(state): State<Arc<ServiceState>>) -> Response {
    response(service(&state).map(|s| Json(s.discovery()).into_response()))
}
async fn jwks(State(state): State<Arc<ServiceState>>) -> Response {
    response(async { Ok(Json(service(&state)?.jwks().await?).into_response()) }.await)
}
fn outcome(value: AuthorizationOutcome) -> Response {
    match value {
        AuthorizationOutcome::Redirect(url) => Redirect::to(&url).into_response(),
        AuthorizationOutcome::LoginRequired => oauth_error(Error::LoginRequired),
        AuthorizationOutcome::ConsentRequired(value) => {
            Json(json!({"consent":value})).into_response()
        }
    }
}
async fn authorize(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let result = async {
        let f = fields(query.as_deref().unwrap_or(""), true)?;
        f.only(&[
            "client_id",
            "redirect_uri",
            "response_type",
            "scope",
            "state",
            "nonce",
            "code_challenge",
            "code_challenge_method",
            "prompt",
            "max_age",
        ])?;
        let request = AuthorizationRequest {
            client_id: f.required("client_id")?,
            redirect_uri: f.required("redirect_uri")?,
            response_type: f.required("response_type")?,
            scope: f.required("scope")?,
            state: f.optional("state"),
            nonce: f.optional("nonce"),
            code_challenge: f.required("code_challenge")?,
            code_challenge_method: f.required("code_challenge_method")?,
            prompt: f.optional("prompt"),
            max_age: f
                .optional("max_age")
                .map(|v| v.parse().map_err(|_| Error::InvalidRequest))
                .transpose()?,
            resources: f.resources,
        };
        let value = service(&state)?
            .authorize(&cookie(&headers)?, request)
            .await?;
        let navigation = headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("text/html"));
        if navigation {
            match value {
                AuthorizationOutcome::LoginRequired => {
                    let mut url = url::Url::parse(&state.config.auth_origin)
                        .map_err(|_| Error::Configuration)?
                        .join("/login")
                        .map_err(|_| Error::Configuration)?;
                    url.query_pairs_mut().append_pair(
                        "returnTo",
                        &format!(
                            "/api/auth/oauth2/authorize?{}",
                            query.as_deref().unwrap_or("")
                        ),
                    );
                    return Ok(Redirect::to(url.as_str()).into_response());
                }
                AuthorizationOutcome::ConsentRequired(value) => {
                    let mut url = url::Url::parse(&state.config.auth_origin)
                        .map_err(|_| Error::Configuration)?
                        .join("/consent")
                        .map_err(|_| Error::Configuration)?;
                    url.query_pairs_mut()
                        .append_pair("request", &value.request_id);
                    return Ok(Redirect::to(url.as_str()).into_response());
                }
                other @ AuthorizationOutcome::Redirect(_) => return Ok(outcome(other)),
            }
        }
        Ok(outcome(value))
    }
    .await;
    if result.is_err()
        && headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("text/html"))
    {
        return Redirect::to("/auth-error").into_response();
    }
    response(result)
}
async fn consent_details(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    response(
        async {
            let f = fields(query.as_deref().unwrap_or(""), false)?;
            f.only(&["request_id"])?;
            Ok(Json(
                service(&state)?
                    .consent_details(&cookie(&headers)?, &f.required("request_id")?)
                    .await?,
            )
            .into_response())
        }
        .await,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Consent {
    request_id: String,
    approved: bool,
}
async fn consent(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Json(input): Json<Consent>,
) -> Response {
    response(
        async {
            let value = service(&state)?
                .consent(&cookie(&headers)?, &input.request_id, input.approved)
                .await?;
            if headers
                .get(header::ACCEPT)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.contains("application/json"))
                && let AuthorizationOutcome::Redirect(url) = &value
            {
                return Ok(Json(json!({"redirect":url})).into_response());
            }
            Ok(outcome(value))
        }
        .await,
    )
}
async fn token(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut response = response(
        async {
            let f = form(&headers, &body)?;
            let credentials = client(&headers, &f)?;
            if state.auth_limiter.check().is_err() {
                return Ok((
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({"error":"temporarily_unavailable"})),
                )
                    .into_response());
            }
            let value = match f.required("grant_type")?.as_str() {
                "authorization_code" => {
                    f.only(&[
                        "client_id",
                        "client_secret",
                        "grant_type",
                        "code",
                        "redirect_uri",
                        "code_verifier",
                    ])?;
                    service(&state)?
                        .exchange_code(CodeExchange {
                            client: credentials,
                            code: f.required("code")?,
                            redirect_uri: f.required("redirect_uri")?,
                            code_verifier: f.required("code_verifier")?,
                        })
                        .await?
                }
                "refresh_token" => {
                    f.only(&[
                        "client_id",
                        "client_secret",
                        "grant_type",
                        "refresh_token",
                        "scope",
                    ])?;
                    service(&state)?
                        .refresh(RefreshExchange {
                            client: credentials,
                            refresh_token: f.required("refresh_token")?,
                            scope: f.optional("scope"),
                        })
                        .await?
                }
                _ => {
                    return Ok((
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error":"unsupported_grant_type"})),
                    )
                        .into_response());
                }
            };
            Ok(Json(value).into_response())
        }
        .await,
    );
    response
        .headers_mut()
        .insert(header::PRAGMA, "no-cache".parse().unwrap());
    response
}
async fn introspect(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    response(
        async {
            let f = form(&headers, &body)?;
            f.only(&["client_id", "client_secret", "token", "token_type_hint"])?;
            Ok(Json(
                service(&state)?
                    .introspect(&client(&headers, &f)?, &f.required("token")?)
                    .await?,
            )
            .into_response())
        }
        .await,
    )
}
async fn revoke(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    response(
        async {
            let f = form(&headers, &body)?;
            f.only(&["client_id", "client_secret", "token", "token_type_hint"])?;
            service(&state)?
                .revoke(&client(&headers, &f)?, &f.required("token")?)
                .await?;
            Ok(StatusCode::OK.into_response())
        }
        .await,
    )
}
async fn userinfo(State(state): State<Arc<ServiceState>>, headers: HeaderMap) -> Response {
    response(
        async { Ok(Json(service(&state)?.userinfo(bearer(&headers)?).await?).into_response()) }
            .await,
    )
}
async fn end_session(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    response(
        async {
            let f = fields(query.as_deref().unwrap_or(""), false)?;
            f.only(&["id_token_hint", "post_logout_redirect_uri", "state"])?;
            let hint = f.optional("id_token_hint").unwrap_or_default();
            match service(&state)?
                .end_session(
                    &cookie(&headers)?,
                    LogoutRequest {
                        id_token_hint: hint,
                        post_logout_redirect_uri: f.optional("post_logout_redirect_uri"),
                        state: f.optional("state"),
                    },
                )
                .await?
            {
                LogoutOutcome::ConfirmationRequired => {
                    if headers
                        .get(header::ACCEPT)
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|v| v.contains("text/html"))
                    {
                        Ok(Redirect::to("/logout").into_response())
                    } else {
                        Ok(Json(json!({"confirmationRequired":true})).into_response())
                    }
                }
                LogoutOutcome::Complete {
                    redirect,
                    set_cookie,
                } => {
                    let mut response = redirect
                        .map(|url| Redirect::to(&url).into_response())
                        .unwrap_or_else(|| Json(json!({"success":true})).into_response());
                    response.headers_mut().append(
                        header::SET_COOKIE,
                        set_cookie.parse().map_err(|_| Error::Configuration)?,
                    );
                    Ok(response)
                }
            }
        }
        .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, header};

    #[test]
    fn duplicate_fields_and_ambiguous_authentication_are_rejected() {
        assert!(fields("code=a&code=b", false).is_err());
        assert!(fields("client_id=a&%63lient_id=b", false).is_err());
        assert!(fields("code=%FF", false).is_err());
        assert!(fields("code=%Q1", false).is_err());
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Basic YTpi".parse().unwrap());
        assert!(client(&headers, &fields("client_id=a", false).unwrap()).is_err());
        let basic = client(
            &headers,
            &fields("grant_type=authorization_code", false).unwrap(),
        )
        .unwrap();
        assert_eq!(basic.client_id, "a");
        assert_eq!(basic.client_secret.as_deref(), Some("b"));
        headers.append(header::AUTHORIZATION, "Basic YTpi".parse().unwrap());
        assert!(client(&headers, &fields("", false).unwrap()).is_err());
    }

    #[test]
    fn repeated_resources_are_distinct_from_duplicate_security_fields() {
        let parsed = fields(
            "resource=https%3A%2F%2Fa.example&resource=https%3A%2F%2Fb.example",
            true,
        )
        .unwrap();
        assert_eq!(parsed.resources.len(), 2);
        assert!(fields("resource=a&resource=b", false).is_err());
        assert!(fields("state=a&state=b", true).is_err());
    }
}
