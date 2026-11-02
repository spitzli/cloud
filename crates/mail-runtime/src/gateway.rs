//! Public native API proxy. Authenticate before polling, then forward once as a bounded stream.
use crate::{SendControl, send_control::ControlError};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{Method, StatusCode},
    response::Response,
    routing::any,
};
use http_body_util::BodyExt;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tonic::transport::Channel;
use tower::ServiceExt;

pub struct Gateway {
    control: Arc<dyn SendControl>,
    runtime: Channel,
    slots: Arc<tokio::sync::Semaphore>,
}
impl Gateway {
    /// Runtime channel must be the operator-selected pinned mTLS connector, never user input.
    pub fn new(control: Arc<dyn SendControl>, runtime: Channel) -> Arc<Self> {
        Arc::new(Self {
            control,
            runtime,
            slots: Arc::new(tokio::sync::Semaphore::new(4)),
        })
    }
    pub fn router(self: Arc<Self>) -> Router {
        Router::new().fallback(any(handle)).with_state(self)
    }
}
async fn handle(State(gateway): State<Arc<Gateway>>, request: Request) -> Response {
    if request.method() == Method::GET
        && matches!(
            request.uri().path_and_query().map(|p| p.as_str()),
            Some("/health" | "/healthz")
        )
    {
        return super::send_api::reply(StatusCode::OK, json!({"ok":true}));
    }
    let Ok(permit) = gateway.slots.clone().try_acquire_owned() else {
        return super::send_api::reply(
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error":"Mail is busy. Try later."}),
        );
    };
    let result = async {
        let token = super::send_api::token(request.headers())?;
        let path = request
            .uri()
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or_default();
        let allowed = (path == "/v1/emails" && request.method() == Method::POST)
            || (path
                .strip_prefix("/v1/emails/")
                .is_some_and(crate::proof::valid_id)
                && matches!(*request.method(), Method::GET | Method::DELETE));
        if !allowed {
            return Err(ControlError { status: 404 });
        }
        // This fixed command is the gateway's only Core capability. No body has been polled.
        let access = gateway
            .control
            .call(json!({"action":"send.authenticate","token":token}))
            .await?;
        if !access["customerID"]
            .as_str()
            .is_some_and(crate::proof::valid_id)
        {
            return Err(ControlError::unavailable());
        }
        let (parts, body) = request.into_parts();
        let mut outgoing = tonic::codegen::http::Request::builder()
            .method(parts.method)
            .uri(
                parts
                    .uri
                    .path_and_query()
                    .map(|p| p.as_str())
                    .unwrap_or_default(),
            );
        for name in ["authorization", "content-type", "idempotency-key"] {
            for value in parts.headers.get_all(name) {
                outgoing = outgoing.header(name, value);
            }
        }
        let body_failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = body_failed.clone();
        let body = Body::new(
            http_body_util::Limited::new(body, 16 * 1024 * 1024).map_err(move |error| {
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
                error
            }),
        );
        let request = outgoing
            .body(tonic::body::Body::new(body))
            .map_err(|_| ControlError::unavailable())?;
        let response = tokio::time::timeout(
            Duration::from_secs(55),
            gateway.runtime.clone().oneshot(request),
        )
        .await
        .map_err(|_| ControlError::unavailable())?
        .map_err(|_| ControlError {
            status: if body_failed.load(std::sync::atomic::Ordering::SeqCst) {
                413
            } else {
                503
            },
        })?;
        let (mut parts, body) = response.into_parts();
        // Never expose transport/private headers; the native API owns its security contract.
        let mut headers = axum::http::HeaderMap::new();
        for name in [
            "content-type",
            "cache-control",
            "x-content-type-options",
            "connection",
        ] {
            if let Some(value) = parts.headers.remove(name) {
                headers.insert(axum::http::HeaderName::from_static(name), value);
            }
        }
        parts.headers = headers;
        // Keep the gateway ingress permit until the response stream ends/disconnects.
        let stream = async_stream_response(body, permit);
        Ok::<_, ControlError>(Response::from_parts(parts, stream))
    }
    .await;
    result.unwrap_or_else(|error| {
        super::send_api::reply(
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
            json!({"error":"Mail request could not be completed."}),
        )
    })
}
fn async_stream_response(
    body: tonic::body::Body,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Body {
    use futures_util::StreamExt;
    use http_body_util::BodyExt;
    let stream = http_body_util::Limited::new(body, 131072)
        .into_data_stream()
        .map(move |chunk| {
            let _ = &permit;
            chunk
        });
    Body::from_stream(stream)
}
