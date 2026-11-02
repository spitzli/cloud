//! Confidential HTTP compatibility transport. No cookies, redirects, automatic retries or proxy
//! environment are inherited. The caller supplies only the scoped Mail gateway credential.
use crate::gateway::{Control, ControlFailure};
use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde_json::Value;
use std::time::Duration;
use url::Url;
pub struct HttpControl {
    client: Client,
    control: Url,
    sending: Url,
}
fn origin(value: &str) -> Result<Url, ControlFailure> {
    let url = Url::parse(value).map_err(|_| ControlFailure)?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ControlFailure);
    }
    Ok(url)
}
impl HttpControl {
    pub fn new(
        auth_origin: &str,
        send_origin: &str,
        secret: &str,
        trust: &[reqwest::Certificate],
    ) -> Result<Self, ControlFailure> {
        let auth = origin(auth_origin)?;
        let send = origin(send_origin)?;
        if auth.scheme() != "https"
            || send.scheme() != "http"
            || send.host_str() != Some("127.0.0.1")
            || !(32..=512).contains(&secret.len())
            || secret.bytes().any(|b| b.is_ascii_whitespace())
        {
            return Err(ControlFailure);
        }
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {secret}")).map_err(|_| ControlFailure)?;
        authorization.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        let mut builder = Client::builder()
            .default_headers(headers)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(5));
        for ca in trust {
            builder = builder.add_root_certificate(ca.clone());
        }
        Ok(Self {
            client: builder.build().map_err(|_| ControlFailure)?,
            control: auth.join("/api/mail/gateway").map_err(|_| ControlFailure)?,
            sending: send.join("/internal/webmail").map_err(|_| ControlFailure)?,
        })
    }
    async fn request(
        &self,
        url: &Url,
        input: Value,
        timeout: Duration,
        max: usize,
    ) -> Result<Value, ControlFailure> {
        let body = serde_json::to_vec(&input).map_err(|_| ControlFailure)?;
        if body.len() > 8192 {
            return Err(ControlFailure);
        }
        let mut response = self
            .client
            .post(url.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .timeout(timeout)
            .send()
            .await
            .map_err(|_| ControlFailure)?;
        if !response.status().is_success()
            || response.content_length().is_some_and(|v| v > max as u64)
        {
            return Err(ControlFailure);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ControlFailure)? {
            if body.len() + chunk.len() > max {
                return Err(ControlFailure);
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| ControlFailure)
    }
}
#[async_trait::async_trait]
impl Control for HttpControl {
    async fn call(&self, input: Value) -> Result<Value, ControlFailure> {
        let mut body = self
            .request(&self.control, input, Duration::from_secs(10), 65536)
            .await?;
        body.as_object_mut()
            .and_then(|b| b.remove("data"))
            .ok_or(ControlFailure)
    }
    async fn send(&self, input: Value) -> Result<Value, ControlFailure> {
        self.request(&self.sending, input, Duration::from_secs(30), 4096)
            .await
    }
}
