//! Public sending ingress. Authenticate before reading payloads, persist privately before enqueue
//! acknowledgement, and retain uncertain staging records for explicit reconciliation.
use crate::{
    payload::PrivatePayloadStore,
    proof::valid_id,
    send_control::{ControlError, SendControl},
};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
};
use serde_json::{Value, json};
use sha2::Sha256;
use std::{sync::Arc, time::Duration};

pub struct SendApi {
    control: Arc<dyn SendControl>,
    store: Arc<PrivatePayloadStore>,
    public: Arc<webdock_mail_runtime::send_api::SendApi>,
    internal_secret: Option<String>,
}
fn fail(status: u16) -> ControlError {
    ControlError { status }
}
fn single<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}
fn reply(status: StatusCode, body: Value) -> Response {
    let mut response = (status, Json(body)).into_response();
    let headers = response.headers_mut();
    headers.insert("cache-control", "no-store".parse().unwrap());
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    if status.is_client_error() || status.is_server_error() {
        headers.insert("connection", "close".parse().unwrap());
    }
    response
}
impl SendApi {
    pub fn new(control: Arc<dyn SendControl>, store: Arc<PrivatePayloadStore>) -> Arc<Self> {
        Arc::new(Self {
            public: webdock_mail_runtime::send_api::SendApi::new(control.clone(), store.clone()),
            control,
            store,
            internal_secret: None,
        })
    }
    pub fn with_internal(
        control: Arc<dyn SendControl>,
        store: Arc<PrivatePayloadStore>,
        secret: String,
    ) -> crate::Result<Arc<Self>> {
        if !(32..=512).contains(&secret.len())
            || !secret.bytes().all(|b| (0x21..=0x7e).contains(&b))
        {
            return Err(crate::Error::Invalid);
        }
        Ok(Arc::new(Self {
            control: control.clone(),
            store: store.clone(),
            public: webdock_mail_runtime::send_api::SendApi::new(control.clone(), store.clone()),
            internal_secret: Some(secret),
        }))
    }
    pub fn router(self: Arc<Self>) -> Router {
        Router::new().fallback(any(handle)).with_state(self)
    }
}
async fn handle(State(api): State<Arc<SendApi>>, request: Request) -> Response {
    if request.method() == Method::GET
        && matches!(
            request.uri().path_and_query().map(|p| p.as_str()),
            Some("/health" | "/healthz")
        )
    {
        return reply(StatusCode::OK, json!({"ok":true}));
    }
    let internal = request.method() == Method::POST
        && (request.uri().path_and_query().map(|p| p.as_str()) == Some("/internal/webmail")
            || request
                .uri()
                .path()
                .strip_prefix("/internal/smtp/")
                .is_some_and(valid_id));
    if !internal {
        return api.public.clone().handle(request).await;
    }
    let Ok(permit) = api.public.ingress_slots().try_acquire_owned() else {
        return reply(
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error":"Mail is busy. Try later."}),
        );
    };
    let _permit = permit;
    if request.method() == Method::POST
        && request.uri().path_and_query().map(|p| p.as_str()) == Some("/internal/webmail")
    {
        let supplied =
            single(request.headers(), "authorization").and_then(|v| v.strip_prefix("Bearer "));
        let authenticated = match (supplied, api.internal_secret.as_deref()) {
            (Some(a), Some(b)) => {
                use hmac::{Hmac, Mac};
                let mut mac = Hmac::<Sha256>::new_from_slice(b.as_bytes()).expect("HMAC key");
                mac.update(b"webmail-ingress");
                let mut candidate = Hmac::<Sha256>::new_from_slice(a.as_bytes()).expect("HMAC key");
                candidate.update(b"webmail-ingress");
                a.len() <= 512 && mac.verify_slice(&candidate.finalize().into_bytes()).is_ok()
            }
            _ => false,
        };
        if !authenticated {
            return reply(StatusCode::UNAUTHORIZED, json!({}));
        }
        if !single(request.headers(), "content-type").is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|v| v.eq_ignore_ascii_case("application/json"))
        }) {
            return reply(StatusCode::UNSUPPORTED_MEDIA_TYPE, json!({}));
        }
        let result = async {
            let body =
                tokio::time::timeout(Duration::from_secs(20), to_bytes(request.into_body(), 2048))
                    .await
                    .map_err(|_| fail(408))?
                    .map_err(|_| fail(413))?;
            let command = serde_json::from_slice(&body).map_err(|_| fail(400))?;
            crate::webmail_send_api::execute(api.control.as_ref(), api.store.clone(), command).await
        }
        .await;
        return match result {
            Ok(value) => reply(StatusCode::OK, value),
            Err(error) => reply(
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
                json!({"error":"Mail request could not be completed."}),
            ),
        };
    }

    if request.method() == Method::POST
        && let Some(customer) = request
            .uri()
            .path()
            .strip_prefix("/internal/smtp/")
            .filter(|id| valid_id(id))
    {
        let customer = customer.to_owned();
        let response = crate::smtp_policy::handle(api.control.as_ref(), &customer, request).await;
        return response;
    }

    reply(
        StatusCode::NOT_FOUND,
        json!({"error":"Mail request could not be completed."}),
    )
}
