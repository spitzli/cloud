use crate::{
    auth,
    http::{ApiFailure, ServiceState, reject},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::json;
use spitzli_access::{AccessService, WaitlistDecision, WaitlistSubmission};
use std::sync::Arc;
fn access(state: &ServiceState) -> Result<&AccessService, ApiFailure> {
    state.access.as_deref().ok_or_else(|| {
        reject(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Access requests are temporarily unavailable.",
        )
    })
}
async fn operator(
    state: &ServiceState,
    headers: &HeaderMap,
) -> Result<spitzli_identity::Principal, ApiFailure> {
    let actor = auth::resolve_actor(state, headers).await?.ok_or_else(|| {
        reject(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "Sign in to continue.",
        )
    })?;
    if actor.read_only()
        || !actor.principal.is_operator()
        || actor.principal.customer_scope().is_some()
    {
        return Err(reject(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "Current operator access is required.",
        ));
    }
    access(state)?
        .require_operator_as(&actor.principal)
        .await
        .map_err(crate::access::failure)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    cursor: Option<String>,
}
pub fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .route("/api/console/waitlist", get(list).post(submit))
        .route("/api/console/waitlist/{id}", axum::routing::post(review))
        .layer(DefaultBodyLimit::max(2048))
}
async fn submit(
    State(state): State<Arc<ServiceState>>,
    Json(input): Json<WaitlistSubmission>,
) -> Response {
    let result = async {
        access(&state)?
            .submit_waitlist(input)
            .await
            .map_err(crate::access::failure)?;
        Ok::<_, ApiFailure>((StatusCode::ACCEPTED, Json(json!({"accepted":true}))))
    }
    .await;
    match result {
        Ok(value) => value.into_response(),
        Err(error) => error.into_response(),
    }
}
async fn list(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> Response {
    let result = async {
        let actor = operator(&state, &headers).await?;
        access(&state)?
            .list_waitlist_as(&actor, page.cursor.as_deref())
            .await
            .map(Json)
            .map_err(crate::access::failure)
    }
    .await;
    match result {
        Ok(value) => value.into_response(),
        Err(error) => error.into_response(),
    }
}
async fn review(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<WaitlistDecision>,
) -> Response {
    let result = async {
        let actor = operator(&state, &headers).await?;
        access(&state)?
            .review_waitlist_as(&actor, &id, input)
            .await
            .map(Json)
            .map_err(crate::access::failure)
    }
    .await;
    match result {
        Ok(value) => value.into_response(),
        Err(error) => error.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use sea_orm::DatabaseConnection;
    use spitzli_core::config::PlatformConfig;
    use std::collections::HashMap;
    use tower::ServiceExt;

    use super::*;

    async fn request(method: &str, path: &str, origin: Option<&str>, body: String) -> Response {
        let app = crate::http::build_app(
            PlatformConfig::from_map(&HashMap::new()).unwrap(),
            DatabaseConnection::default(),
        );
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost:3120")
            .header("content-type", "application/json");
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let response = app
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert!(!response.headers().contains_key("location"));
        response
    }

    #[tokio::test]
    async fn public_intake_requires_json_with_explicit_consent_and_no_admission_fields() {
        for body in [
            json!({"email":"contact@example.test"}),
            json!({"email":"contact@example.test","privacyAccepted":"true"}),
            json!({"email":"contact@example.test","privacyAccepted":true,"status":"approved"}),
            json!({"email":"contact@example.test","privacyAccepted":true,"role":"operator"}),
        ] {
            assert_eq!(
                request(
                    "POST",
                    "/api/console/waitlist",
                    Some("http://localhost:3120"),
                    body.to_string(),
                )
                .await
                .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
    }

    #[tokio::test]
    async fn waitlist_writes_reject_foreign_or_missing_browser_origins() {
        for origin in [None, Some("https://attacker.example")] {
            for (path, body) in [
                (
                    "/api/console/waitlist",
                    json!({"email":"contact@example.test","privacyAccepted":true}),
                ),
                (
                    "/api/console/waitlist/10",
                    json!({"action":"approve","version":1}),
                ),
            ] {
                assert_eq!(
                    request("POST", path, origin, body.to_string())
                        .await
                        .status(),
                    StatusCode::FORBIDDEN
                );
            }
        }
    }

    #[tokio::test]
    async fn public_intake_bounds_untrusted_request_bodies_before_service_access() {
        assert_eq!(
            request(
                "POST",
                "/api/console/waitlist",
                Some("http://localhost:3120"),
                json!({"email":"contact@example.test","name":"x".repeat(2048),"privacyAccepted":true}).to_string(),
            )
            .await
            .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }
}
