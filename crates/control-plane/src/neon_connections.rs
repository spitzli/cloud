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
use std::sync::Arc;
use webdock_provider_connections::{Command, Connections, Error};
pub type NeonConnections = Connections;
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
