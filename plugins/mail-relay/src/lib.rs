//! Standalone relay provider. Never linked into the platform Core.
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashSet, time::Duration};
use webdock_mail_message::{Sender, envelope_address, prepare_message};
use webdock_mail_protocol::{
    LeaseClaims,
    runtime::Handler,
    wire::{SendReceipt, send_receipt::State},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    binding: webdock_plugin_http::HttpBinding,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    sender: Sender,
    to: Vec<String>,
    #[serde(default)]
    cc: Vec<String>,
    #[serde(default)]
    bcc: Vec<String>,
    subject: String,
}
#[derive(Debug)]
struct Failure {
    uncertain: bool,
}
fn rejected() -> Failure {
    Failure { uncertain: false }
}
fn unknown() -> Failure {
    Failure { uncertain: true }
}
#[derive(Default)]
pub struct Relay {
    #[cfg(feature = "local-provider-test")]
    test_origin: Option<url::Url>,
}
impl Relay {
    pub fn new() -> Self {
        Self::default()
    }
    #[cfg(feature = "local-provider-test")]
    pub fn local_fixture(origin: &str) -> Result<Self, &'static str> {
        let url = url::Url::parse(origin).map_err(|_| "Invalid fixture origin")?;
        if url.scheme() != "https"
            || url.host_str() != Some("localhost")
            || url.port().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("Invalid fixture origin");
        }
        let mut client = Self::new();
        client.test_origin = Some(url);
        Ok(client)
    }
    async fn once(
        &self,
        claims: &LeaseClaims,
        metadata: Value,
        mime: Vec<u8>,
        configuration: Value,
    ) -> Result<String, Failure> {
        let configuration: Configuration =
            serde_json::from_value(configuration).map_err(|_| rejected())?;
        let binding = configuration.binding;
        let metadata: Metadata = serde_json::from_value(metadata).map_err(|_| rejected())?;
        if claims.job_id.is_empty()
            || claims.job_id.len() > 19
            || claims.job_id.starts_with('0')
            || !claims.job_id.bytes().all(|b| b.is_ascii_digit())
            || metadata.subject.encode_utf16().count() > 700
            || metadata.subject.chars().any(|c| c < ' ' || c == '\u{7f}')
            || metadata.to.is_empty()
            || metadata.to.len() + metadata.cc.len() + metadata.bcc.len() > 100
        {
            return Err(rejected());
        }
        let origin = url::Url::parse(&binding.origin).map_err(|_| rejected())?;
        let permitted = origin.scheme() == "https"
            && origin.port_or_known_default() == Some(443)
            && matches!(
                origin.host_str(),
                Some("api.eu.turbo-smtp.com" | "api.turbo-smtp.com")
            );
        #[cfg(feature = "local-provider-test")]
        let permitted = permitted
            || self
                .test_origin
                .as_ref()
                .is_some_and(|expected| expected == &origin);
        if !permitted
            || binding
                .headers
                .keys()
                .any(|key| key.eq_ignore_ascii_case("authorization"))
        {
            return Err(rejected());
        }
        for name in ["consumerkey", "consumersecret"] {
            let values = binding
                .headers
                .iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value)
                .collect::<Vec<_>>();
            if values.len() != 1
                || values[0].is_empty()
                || values[0].len() > 4096
                || !values[0].bytes().all(|b| (0x21..=0x7e).contains(&b))
            {
                return Err(rejected());
            }
        }
        let target = webdock_plugin_http::bind_target("POST", "/api/v2/mail/send", binding)
            .map_err(|_| rejected())?;
        let mut seen = HashSet::new();
        let mut groups = Vec::new();
        for group in [&metadata.to, &metadata.cc, &metadata.bcc] {
            let mut addresses = Vec::new();
            for address in group {
                let address = envelope_address(address).map_err(|_| rejected())?;
                if seen.insert(address.clone()) {
                    addresses.push(address);
                }
            }
            groups.push(addresses);
        }
        let recipients = groups.iter().flatten().cloned().collect::<Vec<_>>();
        let prepared =
            prepare_message(&mime, &metadata.sender, &recipients).map_err(|_| rejected())?;
        let raw = std::str::from_utf8(&prepared.raw).map_err(|_| rejected())?;
        let mut body = json!({"from":prepared.from,"to":groups[0].join(","),"subject":metadata.subject,"mime_raw":raw,"reference_id":claims.job_id});
        if !groups[1].is_empty() {
            body["cc"] = json!(groups[1].join(","));
        }
        if !groups[2].is_empty() {
            body["bcc"] = json!(groups[2].join(","));
        }
        let body = serde_json::to_vec(&body).map_err(|_| rejected())?;
        if body.len() >= 24_000_000 {
            return Err(rejected());
        }
        // reference_id is correlation, not provider idempotency. Core owns the attempt fence.
        let response = webdock_plugin_http::send_mail(target, "application/json", body)
            .await
            .map_err(|error| match error {
                webdock_plugin_http::Error::Denied | webdock_plugin_http::Error::Limit => {
                    rejected()
                }
                webdock_plugin_http::Error::Transport => unknown(),
            })?;
        if !(200..300).contains(&response.status) {
            return Err(Failure {
                uncertain: !matches!(response.status, 400 | 401 | 403 | 413 | 429),
            });
        }
        let value: Value = serde_json::from_slice(&response.body).map_err(|_| unknown())?;
        // Arbitrary-precision JSON preserves provider IDs beyond the JavaScript safe-integer range.
        let id = match &value["mid"] {
            Value::String(id) => id.clone(),
            Value::Number(id) => id.to_string(),
            _ => return Err(unknown()),
        };
        if id.is_empty()
            || id.len() > 20
            || id.starts_with('0')
            || !id.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(unknown());
        }
        Ok(id)
    }
}
#[async_trait::async_trait]
impl Handler for Relay {
    async fn submit(
        &self,
        claims: &LeaseClaims,
        metadata: Value,
        mime: Vec<u8>,
        private_configuration: Value,
    ) -> SendReceipt {
        match tokio::time::timeout(
            Duration::from_secs(15),
            self.once(claims, metadata, mime, private_configuration),
        )
        .await
        {
            Ok(Ok(provider_id)) => SendReceipt {
                state: State::Accepted as i32,
                provider_id,
            },
            Ok(Err(Failure { uncertain: false })) => SendReceipt {
                state: State::Rejected as i32,
                provider_id: String::new(),
            },
            _ => SendReceipt {
                state: State::Unknown as i32,
                provider_id: String::new(),
            },
        }
    }
}
