//! Signature-authenticated provider ingress. No browser or Kubernetes authority is accepted here.
use crate::http::{ServiceState, reject};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use std::sync::Arc;
pub(crate) const PATH: &str = "/api/integrations/github/webhook";
pub(crate) fn routes() -> Router<Arc<ServiceState>> {
    Router::new().route(PATH, post(receive))
}
async fn receive(State(state): State<Arc<ServiceState>>, request: Request) -> Response {
    let result = async move {
        state
            .config
            .features
            .require("hosting", spitzli_core::features::OperationKind::Read)
            .map_err(|_| {
                reject(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "GitHub build triggers are unavailable.",
                )
            })?;
        let service = state.build.as_ref().ok_or_else(|| {
            reject(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "GitHub build triggers are unavailable.",
            )
        })?;
        let (signature, delivery, event) = {
            let header = |name| {
                if request.headers().get_all(name).iter().count() != 1 {
                    return Err(reject(
                        StatusCode::BAD_REQUEST,
                        "invalid_input",
                        "Invalid GitHub event headers.",
                    ));
                }
                request
                    .headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        reject(
                            StatusCode::BAD_REQUEST,
                            "invalid_input",
                            "Invalid GitHub event headers.",
                        )
                    })
            };
            let signature = header("x-hub-signature-256")?;
            let delivery = header("x-github-delivery")?;
            let event = header("x-github-event")?;
            (signature, delivery, event)
        };
        let bytes = to_bytes(request.into_body(), spitzli_build::MAX_GITHUB_WEBHOOK_BODY)
            .await
            .map_err(|_| {
                reject(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "body_limit",
                    "GitHub event exceeds its limit.",
                )
            })?;
        let value = service
            .receive_github_webhook(
                &bytes,
                &signature,
                &delivery,
                &event,
                state
                    .config
                    .features
                    .require("hosting", spitzli_core::features::OperationKind::Provision)
                    .is_ok(),
            )
            .await
            .map_err(|e| {
                reject(
                    if e.code == "github_webhook_signature_denied" {
                        StatusCode::UNAUTHORIZED
                    } else {
                        StatusCode::from_u16(e.status())
                            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
                    },
                    e.code,
                    "GitHub event could not be accepted.",
                )
            })?;
        Ok::<_, crate::http::ApiFailure>(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
