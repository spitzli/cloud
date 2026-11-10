//! Same-origin native Neon BYOK commands; no provider secrets enter response/error DTOs.
use crate::{
    auth,
    http::{ApiFailure, ServiceState, reject},
};
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::json;
use spitzli_provider_connections::{Command, Connections, Error};
use std::sync::Arc;
pub type NeonConnections = Connections;

/// Preserve the existing HTTP connection-key priority in every private callback process.
pub fn encryption_key(
    env: &std::collections::HashMap<String, String>,
) -> Result<&str, spitzli_core::ContractError> {
    [
        "SPITZLI_HOSTING_ENCRYPTION_KEY",
        "BETTER_AUTH_SECRET",
        "SPITZLI_AUTH_SECRET",
    ]
    .iter()
    .find_map(|key| env.get(*key).map(String::as_str))
    .ok_or_else(|| spitzli_core::ContractError("Missing Neon connection encryption key".into()))
}

pub fn routes() -> Router<Arc<ServiceState>> {
    Router::new().route("/api/console/byok/neon", post(command))
}
fn failure(error: Error) -> ApiFailure {
    reject(
        StatusCode::from_u16(error.status()).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
        "neon_connection_error",
        &error.to_string(),
    )
}
async fn command(State(state): State<Arc<ServiceState>>, request: Request) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let headers = request.headers();
        if headers
            .iter()
            .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
            .sum::<usize>()
            > 16384
        {
            return Err(reject(
                StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
                "invalid_input",
                "Request headers are too large.",
            ));
        }
        let actor = auth::resolve_actor(&state, headers).await?.ok_or_else(|| {
            reject(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Sign in to continue.",
            )
        })?;
        let service = state
            .neon_connections
            .as_ref()
            .ok_or_else(|| failure(Error::Unavailable))?;
        if headers.get_all(header::CONTENT_TYPE).iter().count() != 1
            || headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(';').next())
                .map(str::trim)
                != Some("application/json")
        {
            return Err(reject(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "invalid_input",
                "JSON is required.",
            ));
        }
        let body = axum::body::to_bytes(request.into_body(), 8192)
            .await
            .map_err(|_| {
                reject(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "invalid_input",
                    "Request is too large.",
                )
            })?;
        let command = Command::from_slice(&body).map_err(failure)?;
        let data = service
            .execute(&actor.principal, &actor.authority, command)
            .await
            .map_err(failure)?;
        Ok(Json(json!({"data":data})).into_response())
    }
    .await;
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

#[cfg(test)]
mod retained_connection_key_tests {
    use super::encryption_key;
    #[test]
    fn private_callbacks_and_http_preserve_the_same_connection_key_priority() {
        let mut env = std::collections::HashMap::from([
            ("SPITZLI_AUTH_SECRET".into(), "fixture-auth".into()),
            ("BETTER_AUTH_SECRET".into(), "fixture-better".into()),
            (
                "SPITZLI_HOSTING_ENCRYPTION_KEY".into(),
                "fixture-hosting".into(),
            ),
        ]);
        assert_eq!(encryption_key(&env).unwrap(), "fixture-hosting");
        env.remove("SPITZLI_HOSTING_ENCRYPTION_KEY");
        assert_eq!(encryption_key(&env).unwrap(), "fixture-better");
        env.remove("BETTER_AUTH_SECRET");
        assert_eq!(encryption_key(&env).unwrap(), "fixture-auth");
        env.clear();
        assert!(encryption_key(&env).is_err());
    }
    #[test]
    fn an_explicit_empty_key_is_not_silently_replaced() {
        let env = std::collections::HashMap::from([
            ("SPITZLI_HOSTING_ENCRYPTION_KEY".into(), String::new()),
            ("BETTER_AUTH_SECRET".into(), "fixture-other".into()),
        ]);
        assert_eq!(encryption_key(&env).unwrap(), "");
    }
}
