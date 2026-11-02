//! Bounded management traffic to an inspected private runtime address only.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{net::Ipv4Addr, time::Duration};
use webdock_supervisor::{ExecutionError, Result};

pub(super) struct Client {
    http: reqwest::Client,
    origin: String,
    authorization: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    method_responses: Vec<(String, Value, String)>,
}
#[derive(Deserialize)]
pub(super) struct Rows<T> {
    pub list: Vec<T>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Account {
    pub id: String,
    pub name: String,
    pub domain_id: String,
    pub roles: AccountRole,
}
#[derive(Deserialize)]
#[serde(tag = "@type", deny_unknown_fields)]
pub(super) enum AccountRole {
    Admin {},
}

#[derive(Deserialize)]
pub(super) struct Domain {
    pub id: String,
    pub name: String,
}
#[derive(Deserialize)]
pub(super) struct Listener {
    pub id: String,
    pub name: String,
}
#[derive(Deserialize)]
struct Edition {
    edition: String,
}
impl Client {
    pub(super) fn new(ip: Ipv4Addr, user: &str, password: &str) -> Result<Self> {
        if !ip.is_private() {
            return Err(ExecutionError::Ownership);
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(3))
                .build()
                .map_err(|_| ExecutionError::Runtime)?,
            origin: format!("http://{ip}:8080"),
            authorization: format!("Basic {}", STANDARD.encode(format!("{user}:{password}"))),
        })
    }
    async fn response<T: DeserializeOwned>(&self, request: reqwest::RequestBuilder) -> Result<T> {
        let mut response = request
            .header(reqwest::header::AUTHORIZATION, &self.authorization)
            .send()
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|size| size > 1_048_576)
        {
            return Err(ExecutionError::Runtime);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ExecutionError::Runtime)?
        {
            if bytes.len() + chunk.len() > 1_048_576 {
                return Err(ExecutionError::Runtime);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| ExecutionError::Runtime)
    }
    #[cfg(test)]
    pub(super) fn local_fixture(address: std::net::SocketAddr) -> Result<Self> {
        if !address.ip().is_loopback() {
            return Err(ExecutionError::Ownership);
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(3))
                .build()
                .map_err(|_| ExecutionError::Runtime)?,
            origin: format!("http://{address}"),
            authorization: "Basic c3ludGhldGljOnRlc3Q=".into(),
        })
    }
    pub(super) async fn call<T: DeserializeOwned>(&self, method: &str, args: Value) -> Result<T> {
        let mut response:Envelope=self.response(self.http.post(format!("{}/jmap",self.origin)).json(&json!({"using":["urn:ietf:params:jmap:core","urn:stalwart:jmap"],"methodCalls":[[method,args,"webdock"]]}))).await?;
        if response.method_responses.len() != 1 {
            return Err(ExecutionError::Runtime);
        }
        let (name, value, id) = response.method_responses.remove(0);
        if name != method
            || id != "webdock"
            || !value.is_object()
            || ["notCreated", "notUpdated", "notDestroyed"]
                .iter()
                .any(|key| value[*key].as_object().is_some_and(|v| !v.is_empty()))
        {
            return Err(ExecutionError::Runtime);
        }
        serde_json::from_value(value).map_err(|_| ExecutionError::Runtime)
    }
    pub(super) async fn certificate_pin(&self, hostname: &str) -> Result<String> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Settings {
            default_certificate_id: Option<String>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Certificate {
            id: String,
            certificate: PublicText,
            subject_alternative_names: std::collections::HashMap<String, bool>,
        }
        #[derive(Deserialize)]
        #[serde(tag = "@type")]
        enum PublicText {
            Text { value: String },
        }
        let settings: Rows<Settings> = self
            .call("x:SystemSettings/get", json!({"ids":["singleton"]}))
            .await?;
        let id = settings
            .list
            .first()
            .and_then(|s| s.default_certificate_id.as_ref())
            .ok_or(ExecutionError::Ownership)?;
        let certificates: Rows<Certificate> =
            self.call("x:Certificate/get", json!({"ids":[id]})).await?;
        if certificates.list.len() != 1 {
            return Err(ExecutionError::Ownership);
        }
        let certificate = &certificates.list[0];
        if &certificate.id != id
            || certificate.subject_alternative_names.get(hostname) != Some(&true)
        {
            return Err(ExecutionError::Ownership);
        }
        let PublicText::Text { value } = &certificate.certificate;
        let (_, pem) = x509_parser::pem::parse_x509_pem(value.as_bytes())
            .map_err(|_| ExecutionError::Ownership)?;
        use sha2::{Digest, Sha256};
        Ok(Sha256::digest(pem.contents)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }
    pub(super) async fn identity_access(
        &self,
        username: &str,
        password: &str,
        account: &str,
        enabled: bool,
    ) -> Result<()> {
        let identity = Self {
            http: self.http.clone(),
            origin: self.origin.clone(),
            authorization: format!(
                "Basic {}",
                STANDARD.encode(format!("{username}:{password}"))
            ),
        };
        let request = identity
            .http
            .get(format!("{}/jmap/session", identity.origin));
        if !enabled {
            let response = request
                .header(reqwest::header::AUTHORIZATION, &identity.authorization)
                .send()
                .await
                .map_err(|_| ExecutionError::Runtime)?;
            return match response.status() {
                reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => Ok(()),
                status if status.is_success() => Err(ExecutionError::Ownership),
                _ => Err(ExecutionError::Runtime),
            };
        }
        #[derive(Deserialize)]
        struct Session {
            accounts: std::collections::BTreeMap<String, serde::de::IgnoredAny>,
        }
        let session: Session = identity.response(request).await?;
        if !session.accounts.contains_key(account) {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
    pub(super) async fn verify(&self) -> Result<()> {
        let _: Rows<Domain> = self.call("x:Domain/get", json!({"ids":null})).await?;
        let edition: Edition = self
            .response(self.http.get(format!("{}/api/account", self.origin)))
            .await
            .map_err(|_| ExecutionError::Invalid("Mail management edition"))?;
        if edition.edition != "community" {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
}
