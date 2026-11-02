use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde_json::Value;
use std::time::Duration;
#[derive(Debug, thiserror::Error)]
#[error("Mail access is unavailable")]
pub struct ControlError {
    pub status: u16,
}
impl ControlError {
    pub fn unavailable() -> Self {
        Self { status: 503 }
    }
    pub fn invalid() -> Self {
        Self { status: 400 }
    }
}
#[async_trait::async_trait]
pub trait SendControl: Send + Sync {
    async fn call(&self, input: Value) -> Result<Value, ControlError>;
}
pub struct HttpSendControl {
    client: Client,
    target: url::Url,
}
impl HttpSendControl {
    pub fn new(
        origin: &str,
        secret: &str,
        trust: &[reqwest::Certificate],
    ) -> Result<Self, ControlError> {
        let origin = url::Url::parse(origin).map_err(|_| ControlError::unavailable())?;
        if origin.scheme() != "https"
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
            || !(32..=512).contains(&secret.len())
            || !secret.bytes().all(|b| (0x21..=0x7e).contains(&b))
        {
            return Err(ControlError::unavailable());
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {secret}"))
            .map_err(|_| ControlError::unavailable())?;
        authorization.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        let mut builder = Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5));
        for ca in trust {
            builder = builder.add_root_certificate(ca.clone());
        }
        Ok(Self {
            client: builder.build().map_err(|_| ControlError::unavailable())?,
            target: origin
                .join("/api/mail/gateway")
                .map_err(|_| ControlError::unavailable())?,
        })
    }
}
#[async_trait::async_trait]
impl SendControl for HttpSendControl {
    async fn call(&self, input: Value) -> Result<Value, ControlError> {
        let body = serde_json::to_vec(&input).map_err(|_| ControlError::invalid())?;
        if body.len() > 65536 {
            return Err(ControlError::invalid());
        }
        let mut response = self
            .client
            .post(self.target.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| ControlError::unavailable())?;
        if !response.status().is_success() {
            return Err(ControlError {
                status: match response.status().as_u16() {
                    status @ (400 | 401 | 403) => status,
                    _ => 503,
                },
            });
        }
        if response.content_length().is_some_and(|n| n > 131072) {
            return Err(ControlError::unavailable());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ControlError::unavailable())?
        {
            if body.len() + chunk.len() > 131072 {
                return Err(ControlError::unavailable());
            }
            body.extend_from_slice(&chunk);
        }
        let mut value: Value =
            serde_json::from_slice(&body).map_err(|_| ControlError::unavailable())?;
        value
            .as_object_mut()
            .and_then(|o| o.remove("data"))
            .ok_or_else(ControlError::unavailable)
    }
}

#[async_trait::async_trait]
impl SendControl for webdock_mail_protocol::native_control::NativeControlClient {
    async fn call(&self, input: Value) -> Result<Value, ControlError> {
        webdock_mail_protocol::native_control::NativeControlClient::call(self, input)
            .await
            .map_err(|error| ControlError {
                status: error.status,
            })
    }
}
