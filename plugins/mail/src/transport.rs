//! Delivery: the platform transport from the private configuration, or a project's own turboSMTP account.
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

/// EU region for every provider call.
pub const TURBOSMTP_ORIGIN: &str = "https://api.eu.turbo-smtp.com";

#[derive(Clone, serde::Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Platform {
    /// Plain SMTP to a loopback catcher (local dev); nothing leaves the machine.
    #[serde(rename_all = "camelCase")]
    Smtp { host: String, port: u16 },
    /// The operator's own turboSMTP account.
    #[serde(rename = "turbosmtp", rename_all = "camelCase")]
    TurboSmtp {
        consumer_key: String,
        consumer_secret: String,
    },
}
impl Platform {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Smtp { host, .. }
                if !["127.0.0.1", "::1", "localhost"].contains(&host.as_str()) =>
            {
                Err("plain SMTP transport is for a loopback catcher only")
            }
            Self::Smtp { port: 0, .. } => Err("SMTP catcher port required"),
            Self::TurboSmtp {
                consumer_key,
                consumer_secret,
            } if [consumer_key, consumer_secret].iter().any(|s| {
                s.is_empty() || s.len() > 512 || !s.bytes().all(|b| (0x21..=0x7e).contains(&b))
            }) =>
            {
                Err("invalid private platform credentials")
            }
            _ => Ok(()),
        }
    }
}
pub enum Route<'a> {
    Platform(&'a Platform),
    Own { key: String, secret: String },
}
pub struct Sender {
    http: reqwest::Client,
    origin: String,
}
impl Sender {
    pub fn new(origin: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .timeout(Duration::from_secs(15))
                .build()
                .expect("static HTTP client configuration"),
            origin: origin.into(),
        }
    }
}

impl Sender {
    /// One attempt only. SMTP correlation is internal, never a turboSMTP message ID.
    pub async fn submit_prepared(
        &self,
        route: Route<'_>,
        job: &webdock_mail_runtime::outbox::SendLease,
        message: &webdock_mail_runtime::message::DispatchMessage,
        prepared: &webdock_mail_runtime::message::PreparedMessage,
    ) -> webdock_mail_runtime::outbox::Receipt {
        use webdock_mail_runtime::outbox::Receipt;
        let (key, secret) = match route {
            Route::Platform(Platform::Smtp { host, port }) => {
                return match webdock_mail_runtime::smtp::submit_loopback(
                    host,
                    *port,
                    &prepared.from,
                    &prepared.to,
                    &prepared.raw,
                    Duration::from_secs(15),
                )
                .await
                {
                    Ok(()) => Receipt::Accepted {
                        provider_id: format!("internal_submission_{}", job.id),
                    },
                    Err(error) if error.uncertain => Receipt::Unknown {},
                    Err(_) => Receipt::Rejected {},
                };
            }
            Route::Platform(Platform::TurboSmtp {
                consumer_key,
                consumer_secret,
            }) => (consumer_key.clone(), consumer_secret.clone()),
            Route::Own { key, secret } => (key, secret),
        };
        if [&key, &secret].iter().any(|s| {
            s.is_empty() || s.len() > 4096 || !s.bytes().all(|b| (0x21..=0x7e).contains(&b))
        }) {
            return Receipt::Rejected {};
        }
        let Ok(raw) = std::str::from_utf8(&prepared.raw) else {
            return Receipt::Rejected {};
        };
        let mut body = json!({"from":prepared.from,"to":message.to.join(","),"subject":message.subject,"mime_raw":raw,"reference_id":job.id});
        if !message.cc.is_empty() {
            body["cc"] = json!(message.cc.join(","));
        }
        if !message.bcc.is_empty() {
            body["bcc"] = json!(message.bcc.join(","));
        }
        let Ok(body) = serde_json::to_vec(&body) else {
            return Receipt::Rejected {};
        };
        if body.len() > 2 * webdock_mail_protocol::MAX_MIME_BYTES as usize + 65536 {
            return Receipt::Rejected {};
        }
        let response = self
            .http
            .post(format!("{}/api/v2/mail/send", self.origin))
            .header("consumerKey", key)
            .header("consumerSecret", secret)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await;
        let Ok(mut response) = response else {
            return Receipt::Unknown {};
        };
        if !response.status().is_success() {
            return if matches!(response.status().as_u16(), 400 | 401 | 403 | 413 | 429) {
                Receipt::Rejected {}
            } else {
                Receipt::Unknown {}
            };
        }
        if response.content_length().is_some_and(|n| n > 65536) {
            return Receipt::Unknown {};
        }
        let mut bytes = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) if bytes.len() + chunk.len() <= 65536 => {
                    bytes.extend_from_slice(&chunk)
                }
                Ok(None) => break,
                _ => return Receipt::Unknown {},
            }
        }
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            return Receipt::Unknown {};
        };
        let id = match &value["mid"] {
            Value::String(id) => id.clone(),
            Value::Number(id) => id.to_string(),
            _ => return Receipt::Unknown {},
        };
        if id.is_empty()
            || id.len() > 20
            || id.starts_with('0')
            || !id.bytes().all(|b| b.is_ascii_digit())
        {
            return Receipt::Unknown {};
        }
        Receipt::Accepted { provider_id: id }
    }
}
