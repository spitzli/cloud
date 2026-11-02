//! Provider-neutral account notifications enter the native Mail queue using a reviewed key.
//! The durable account outboxes own retries; this client never retries a network request itself.
use crate::message::{Sender, envelope_address};
use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use sea_orm::{Database, DatabaseConnection};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use webdock_identity::{
    CookiePolicy, Delivery, DeliveryError, DeliveryKind, DeliveryRequest, IdentityService,
    RlsSigner, SecretKeys,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MailConfiguration {
    pub origin: String,
    pub authority: Option<PathBuf>,
    pub from: Sender,
    pub api_key: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DispatcherConfiguration {
    pub auth_database_url: String,
    pub business_database_url: String,
    pub auth_secret: String,
    pub rls_secret: String,
    pub account_origin: String,
    pub mail: MailConfiguration,
}
pub struct MailDelivery {
    transport: Transport,
    account_origin: url::Url,
    from: String,
    sender: String,
}
enum Transport {
    Api {
        client: Client,
        target: url::Url,
    },
    /// Plain SMTP to a local catch-all (development mail viewer); never a remote relay.
    Loopback(std::net::SocketAddr),
}
fn unavailable(_: impl std::fmt::Display) -> DeliveryError {
    DeliveryError::Unavailable
}
fn canonical_id(value: &str) -> bool {
    webdock_core::id::RecordId::parse(value).is_ok()
}
fn origin(value: &str) -> Result<url::Url, DeliveryError> {
    let parsed = url::Url::parse(value).map_err(unavailable)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(DeliveryError::Unavailable);
    }
    Ok(parsed)
}
fn account_origin(value: &str) -> Result<url::Url, DeliveryError> {
    let url = url::Url::parse(value).map_err(unavailable)?;
    webdock_identity::validate_account_origin(&url).map_err(unavailable)?;
    Ok(url)
}
/// Configuration and trust files are operator-owned; no caller can substitute these paths.
pub fn read_configuration(path: &Path) -> Result<Vec<u8>, DeliveryError> {
    if !path.is_absolute() {
        return Err(DeliveryError::Unavailable);
    }
    let file = std::fs::File::from(
        rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(unavailable)?,
    );
    let metadata = file.metadata().map_err(unavailable)?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
        || metadata.len() > 65536
    {
        return Err(DeliveryError::Unavailable);
    }
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(unavailable)?;
    if bytes.len() > 65536 {
        return Err(DeliveryError::Unavailable);
    }
    Ok(bytes)
}
impl MailDelivery {
    pub fn new(config: MailConfiguration, account_origin: &str) -> Result<Self, DeliveryError> {
        let target = origin(&config.origin)?
            .join("/v1/emails")
            .map_err(unavailable)?;
        let account_origin = self::account_origin(account_origin)?;
        let suffix = config
            .api_key
            .strip_prefix("wd_mail_")
            .ok_or(DeliveryError::Unavailable)?;
        if suffix.len() != 43
            || !suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            || envelope_address(&config.from.email).map_err(unavailable)? != config.from.email
            || config.from.name.as_ref().is_some_and(|name| {
                name.trim().is_empty()
                    || name.contains(['<', '>'])
                    || name.encode_utf16().count() > 80
                    || name.chars().any(|c| c < ' ' || c == '\u{7f}')
            })
        {
            return Err(DeliveryError::Unavailable);
        }
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {}", config.api_key)).map_err(unavailable)?;
        authorization.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        let mut builder = Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(8));
        if let Some(path) = config.authority {
            let raw = read_configuration(&path)?;
            if raw.windows(11).any(|bytes| bytes == b"PRIVATE KEY") {
                return Err(DeliveryError::Unavailable);
            }
            let certificates = reqwest::Certificate::from_pem_bundle(&raw).map_err(unavailable)?;
            if certificates.is_empty() || certificates.len() > 32 {
                return Err(DeliveryError::Unavailable);
            }
            for certificate in certificates {
                builder = builder.add_root_certificate(certificate);
            }
        }
        Ok(Self {
            transport: Transport::Api {
                client: builder.build().map_err(unavailable)?,
                target,
            },
            account_origin,
            sender: config.from.email.clone(),
            from: match config.from.name {
                Some(name) => format!("{name} <{}>", config.from.email),
                None => config.from.email,
            },
        })
    }
    pub fn loopback(
        smtp: std::net::SocketAddr,
        from: &str,
        account_origin: &str,
    ) -> Result<Self, DeliveryError> {
        if !smtp.ip().is_loopback() || envelope_address(from).map_err(unavailable)? != from {
            return Err(DeliveryError::Unavailable);
        }
        Ok(Self {
            transport: Transport::Loopback(smtp),
            account_origin: self::account_origin(account_origin)?,
            from: from.into(),
            sender: from.into(),
        })
    }
    async fn loopback_send(
        &self,
        smtp: std::net::SocketAddr,
        id: &str,
        message: Message<'_>,
    ) -> Result<(), DeliveryError> {
        let raw = format!(
            "From: {}\r\nTo: <{}>\r\nSubject: {}\r\nDate: {}\r\nMessage-ID: <account-{id}@webdock.invalid>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{}\r\n",
            message.from,
            message.to[0],
            message.subject,
            chrono::Utc::now().to_rfc2822(),
            message.text.replace('\n', "\r\n")
        );
        let timeout = Duration::from_secs(8);
        tokio::time::timeout(timeout, async {
            use mail_send::smtp::AssertReply;
            let stream = tokio::net::TcpStream::connect(smtp)
                .await
                .map_err(unavailable)?;
            let mut client = mail_send::SmtpClient { stream, timeout };
            client
                .read()
                .await
                .map_err(unavailable)?
                .assert_positive_completion()
                .map_err(unavailable)?;
            client.ehlo("webdock.invalid").await.map_err(unavailable)?;
            for command in [
                format!("MAIL FROM:<{}>\r\n", self.sender),
                format!("RCPT TO:<{}>\r\n", message.to[0]),
            ] {
                client
                    .cmd(command)
                    .await
                    .map_err(unavailable)?
                    .assert_positive_completion()
                    .map_err(unavailable)?;
            }
            client
                .cmd(b"DATA\r\n")
                .await
                .map_err(unavailable)?
                .assert_code(354)
                .map_err(unavailable)?;
            client
                .write_message(raw.as_bytes())
                .await
                .map_err(unavailable)?;
            client
                .read()
                .await
                .map_err(unavailable)?
                .assert_positive_completion()
                .map_err(unavailable)?;
            let _ = client.cmd(b"QUIT\r\n").await;
            Ok(())
        })
        .await
        .map_err(unavailable)?
    }
    fn message(&self, request: &DeliveryRequest) -> Result<Message<'_>, DeliveryError> {
        if !canonical_id(&request.id) || request.url.len() > 2048 {
            return Err(DeliveryError::Unavailable);
        }
        let to = envelope_address(&request.email).map_err(unavailable)?;
        let link = url::Url::parse(&request.url).map_err(unavailable)?;
        if link.origin() != self.account_origin.origin()
            || !link.username().is_empty()
            || link.password().is_some()
            || link.fragment().is_some()
        {
            return Err(DeliveryError::Unavailable);
        }
        let query = link.query_pairs().collect::<Vec<_>>();
        let path = match request.kind {
            DeliveryKind::Reset | DeliveryKind::Activation => {
                // An activation may carry the invitation it completes.
                let invitation = query.len() == 2
                    && request.kind == DeliveryKind::Activation
                    && query[1].0 == "invitation"
                    && canonical_id(&query[1].1);
                if !(query.len() == 1 || invitation)
                    || query[0].0 != "token"
                    || query[0].1.len() != 64
                    || !query[0]
                        .1
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(DeliveryError::Unavailable);
                }
                "/reset-password"
            }
            DeliveryKind::Invitation => {
                if query.len() != 1 || query[0].0 != "id" || !canonical_id(&query[0].1) {
                    return Err(DeliveryError::Unavailable);
                }
                "/invitation"
            }
            DeliveryKind::WebsiteAccess => {
                if !query.is_empty() {
                    return Err(DeliveryError::Unavailable);
                }
                "/sites"
            }
            DeliveryKind::Notice => {
                if !query.is_empty() {
                    return Err(DeliveryError::Unavailable);
                }
                "/"
            }
        };
        let mut expected = self.account_origin.join(path).map_err(unavailable)?;
        for (name, value) in &query {
            expected.query_pairs_mut().append_pair(name, value);
        }
        if expected.as_str() != request.url {
            return Err(DeliveryError::Unavailable);
        }
        let (subject, text) = match request.kind {
            DeliveryKind::Activation => (
                "Set up your Spitzli Cloud account",
                format!(
                    "Your Spitzli Cloud administrator has invited you. Choose your password to set up your account:\n{}\nIf you were not expecting an invitation, you can ignore this message.",
                    request.url
                ),
            ),
            DeliveryKind::Reset => (
                "Reset your Spitzli Cloud password",
                format!(
                    "Reset your Spitzli Cloud password using this link:\n{}\nIf you did not request this, you can ignore this message.",
                    request.url
                ),
            ),
            DeliveryKind::Invitation => (
                "Invitation to Spitzli Cloud",
                format!("You have been invited to Spitzli Cloud.\n{}", request.url),
            ),
            DeliveryKind::Notice => (
                "A service in your Spitzli Cloud workspace needs attention",
                format!(
                    "A service in one of your Spitzli Cloud workspaces has needed attention for a few minutes, for example because it keeps stopping or its image cannot be pulled. Sign in to see which one and why:\n{}",
                    request.url
                ),
            ),
            DeliveryKind::WebsiteAccess => (
                "Your Spitzli Cloud website access",
                format!(
                    "Your administrator has granted you website access on Spitzli Cloud.\n{}",
                    request.url
                ),
            ),
        };
        Ok(Message {
            from: &self.from,
            to: [to],
            subject,
            text,
        })
    }
}
#[derive(Serialize)]
struct Message<'a> {
    from: &'a str,
    to: [String; 1],
    subject: &'static str,
    text: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum MailState {
    Staging,
    Queued,
    Leased,
    PreparationReview,
    Handoff,
    Unknown,
    Accepted,
    Rejected,
    Cancelled,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    id: String,
    state: MailState,
}
#[async_trait::async_trait]
impl Delivery for MailDelivery {
    async fn deliver(&self, request: &DeliveryRequest) -> Result<(), DeliveryError> {
        let message = self.message(request)?;
        let (client, target) = match &self.transport {
            Transport::Api { client, target } => (client, target),
            Transport::Loopback(smtp) => {
                return self.loopback_send(*smtp, &request.id, message).await;
            }
        };
        let mut response = client
            .post(target.clone())
            .header("idempotency-key", format!("account:{}", request.id))
            .json(&message)
            .send()
            .await
            .map_err(unavailable)?;
        if response.status() != reqwest::StatusCode::ACCEPTED
            || response.content_length().is_some_and(|n| n > 4096)
            || !response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| {
                    v.split(';')
                        .next()
                        .is_some_and(|v| v.eq_ignore_ascii_case("application/json"))
                })
        {
            return Err(DeliveryError::Unavailable);
        }
        let mut raw = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(unavailable)? {
            if raw.len() + chunk.len() > 4096 {
                return Err(DeliveryError::Unavailable);
            }
            raw.extend_from_slice(&chunk);
        }
        let receipt: Receipt = serde_json::from_slice(&raw).map_err(unavailable)?;
        if !canonical_id(&receipt.id)
            || !matches!(
                receipt.state,
                MailState::Queued
                    | MailState::Leased
                    | MailState::Handoff
                    | MailState::Unknown
                    | MailState::Accepted
            )
        {
            return Err(DeliveryError::Unavailable);
        }
        Ok(())
    }
}
pub struct AccountDispatcher {
    identity: Arc<IdentityService>,
    access: webdock_access::AccessService,
}
impl AccountDispatcher {
    pub async fn connect(config: DispatcherConfiguration) -> Result<Self, DeliveryError> {
        let mail = Arc::new(MailDelivery::new(config.mail, &config.account_origin)?);
        let auth = restricted(&config.auth_database_url, "webdock_auth_bootstrap").await?;
        let business = restricted(&config.business_database_url, "webdock_auth_runtime").await?;
        let secret_keys = || {
            SecretKeys::new(
                BTreeMap::from([(1, config.auth_secret.clone())]),
                1,
                Some(config.auth_secret.clone()),
            )
            .map_err(unavailable)
        };
        let identity = Arc::new(
            IdentityService::new(
                auth.clone(),
                CookiePolicy::new(
                    config.account_origin.starts_with("https://"),
                    config.auth_secret.clone(),
                )
                .map_err(unavailable)?,
            )
            .with_secret_keys(secret_keys()?)
            .with_delivery(mail.clone()),
        );
        let allocator = Arc::new(webdock_persistence::IdAllocator::new(
            business.clone(),
            webdock_ids::Identity::platform(0, 2).map_err(unavailable)?,
        ));
        let access = webdock_access::AccessService::new(
            business,
            auth,
            identity.clone(),
            allocator,
            RlsSigner::new(config.rls_secret, "webdock_auth_runtime").map_err(unavailable)?,
            Arc::new(secret_keys()?),
            webdock_access::AccessConfig {
                auth_origin: account_origin(&config.account_origin)?,
                console_origins: vec![],
                allow_local_http: true,
            },
        )
        .map_err(unavailable)?
        .with_delivery(mail);
        Ok(Self { identity, access })
    }
    pub async fn dispatch_once(&self) -> Result<(), DeliveryError> {
        let (identity, access) = tokio::join!(
            self.identity.dispatch_delivery_jobs(1),
            self.access.dispatch_invitation_jobs(1)
        );
        let identity = identity.map_err(unavailable)?;
        let access = access.map_err(unavailable)?;
        if identity.failed > 0 || access.failed > 0 {
            return Err(DeliveryError::Unavailable);
        }
        Ok(())
    }
}
async fn restricted(url: &str, role: &str) -> Result<DatabaseConnection, DeliveryError> {
    let url =
        webdock_persistence::connection::ConnectionUrl::parse(url, false).map_err(unavailable)?;
    let mut options = sea_orm::ConnectOptions::new(url.as_str().to_owned());
    options
        .max_connections(4)
        .min_connections(0)
        .connect_timeout(Duration::from_secs(5))
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(60))
        .sqlx_logging(false);
    let db = Database::connect(options).await.map_err(unavailable)?;
    webdock_persistence::connection::verify_runtime_role(&db, role)
        .await
        .map_err(unavailable)?;
    Ok(db)
}
