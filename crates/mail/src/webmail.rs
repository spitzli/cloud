//! Native session authority and the retained Webmail launch protocol. Gateway methods are backend
//! only: the HTTP adapter must authenticate its scoped machine credential before calling them.
use crate::{
    Error, Result,
    outbox::{field, one, run},
    proof::valid_id,
    secrets::MailCipher,
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{DateTime, Utc};
use sea_orm::{DatabaseConnection, DatabaseTransaction, QueryResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use webdock_browser_session::{BrowserSessions, StoredAuthority};
use webdock_identity::{IdentityService, Principal, RlsSigner};
use webdock_persistence::IdAllocator;

#[derive(Clone)]
pub struct WebmailDependencies {
    pub auth: DatabaseConnection,
    pub business: DatabaseConnection,
    pub worker: DatabaseConnection,
    pub identity: Arc<IdentityService>,
    pub browser: Option<Arc<BrowserSessions>>,
    pub ids: Arc<IdAllocator>,
}
pub struct WebmailConfig {
    pub enabled: bool,
    pub gateway_origin: String,
    pub managed_cluster: String,
    pub rls_secret: String,
    pub encryption_key: String,
}
pub struct WebmailService {
    dependencies: WebmailDependencies,
    enabled: bool,
    gateway_origin: String,
    managed_cluster: String,
    rls_secret: String,
    signer: RlsSigner,
    cipher: MailCipher,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Launch {
    pub code: String,
    pub gateway_origin: String,
    #[serde(rename = "mailboxID")]
    pub mailbox_id: String,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    pub expires_at: DateTime<Utc>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exchanged {
    pub token: String,
    #[serde(rename = "scopeID")]
    pub scope_id: String,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "mailboxID")]
    pub mailbox_id: String,
    pub expires_at: DateTime<Utc>,
}
/// Machine-only output. Never use this as the public browser session response.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GatewayAccess {
    #[serde(rename = "scopeID")]
    pub scope_id: String,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "mailboxID")]
    pub mailbox_id: String,
    #[serde(rename = "accountID")]
    pub account_id: String,
    pub sender_name: String,
    pub sender_address: String,
    pub runtime_origin: String,
    pub authorization: String,
    pub subject: String,
    pub grant_revision: String,
    pub expires_at: DateTime<Utc>,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WriteAction {
    Draft,
    Submit,
    Update,
}
impl WriteAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Submit => "submit",
            Self::Update => "update",
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReserveWrite {
    pub token: String,
    #[serde(rename = "requestID")]
    pub request_id: uuid::Uuid,
    pub action: WriteAction,
    #[serde(rename = "resourceID")]
    pub resource_id: Option<String>,
    pub payload_hash: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteReservation {
    pub execute: bool,
    #[serde(rename = "operationID")]
    pub operation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_token: Option<uuid::Uuid>,
    pub state: String,
    pub result: Option<Value>,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WriteState {
    Succeeded,
    Rejected,
    Uncertain,
}
impl WriteState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Rejected => "rejected",
            Self::Uncertain => "uncertain",
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FinishWrite {
    pub token: String,
    #[serde(rename = "operationID")]
    pub operation_id: String,
    pub lease_token: uuid::Uuid,
    pub state: WriteState,
    pub result: Option<WriteResult>,
}
fn resource(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
pub(crate) struct Authorized {
    auth: DatabaseTransaction,
    business: DatabaseTransaction,
    principal: Principal,
    mailbox: Mailbox,
}
struct Mailbox {
    id: String,
    name: String,
    account: String,
    domain: String,
    local_part: String,
    identity: String,
    revision: i64,
}
struct Bound {
    id: Option<String>,
    customer: String,
    mailbox: String,
    subject: String,
    session: String,
    revision: i64,
    expires: DateTime<Utc>,
    authority: StoredAuthority,
}
impl Bound {
    fn read(row: &QueryResult, session: bool) -> Result<Self> {
        let authority: Value =
            field::<Option<Value>>(row, "native_authority")?.ok_or(Error::Unavailable)?;
        Ok(Self {
            id: if session {
                Some(field(row, "id")?)
            } else {
                None
            },
            customer: field(row, "customer_id")?,
            mailbox: field(row, "mailbox_id")?,
            subject: field(row, "subject")?,
            session: field(row, "session_id")?,
            revision: field(row, "grant_revision")?,
            expires: field(row, "expires_at")?,
            authority: serde_json::from_value(authority).map_err(|_| Error::Unavailable)?,
        })
    }
}
fn token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| Error::Unavailable)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn hash(value: &str) -> Result<String> {
    if value.len() != 43
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(Error::Invalid);
    }
    Ok(hex::encode(Sha256::digest(value.as_bytes())))
}
impl WebmailService {
    /// Public browser destination only; runtime origins and credentials remain confidential.
    pub fn gateway_origin(&self) -> Option<&str> {
        self.enabled.then_some(self.gateway_origin.as_str())
    }
    pub async fn new(dependencies: WebmailDependencies, config: WebmailConfig) -> Result<Self> {
        let origin = url::Url::parse(&config.gateway_origin).map_err(|_| Error::Invalid)?;
        if origin.scheme() != "https"
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
            || !valid_id(&config.managed_cluster)
        {
            return Err(Error::Invalid);
        }
        crate::authority::validate_pools(&dependencies).await?;
        Ok(Self {
            signer: RlsSigner::new(config.rls_secret.clone(), "webdock_auth_runtime")
                .map_err(|_| Error::Invalid)?,
            cipher: MailCipher::new(&config.encryption_key)?,
            dependencies,
            enabled: config.enabled,
            gateway_origin: origin.origin().ascii_serialization(),
            managed_cluster: config.managed_cluster,
            rls_secret: config.rls_secret,
        })
    }
    async fn worker(&self) -> Result<DatabaseTransaction> {
        crate::authority::worker(
            &self.dependencies,
            &self.rls_secret,
            crate::authority::Purpose::Webmail,
            None,
        )
        .await
    }
    async fn module(&self, tx: &DatabaseTransaction, write: bool) -> Result<()> {
        if !self.enabled {
            return Err(Error::Unavailable);
        }
        crate::authority::module(tx, write).await
    }
    async fn authorize(
        &self,
        worker: &DatabaseTransaction,
        subject: &str,
        session: &str,
        authority: &StoredAuthority,
        customer: &str,
        mailbox: &str,
    ) -> Result<Authorized> {
        if !valid_id(customer) || !valid_id(mailbox) || authority.read_only() {
            return Err(Error::Unavailable);
        }

        if !self.enabled {
            return Err(Error::Unavailable);
        }
        let crate::authority::AuthorizedCustomer {
            auth,
            business,
            principal,
            operator,
            member_id,
        } = crate::authority::customer(
            &self.dependencies,
            &self.signer,
            subject,
            session,
            authority,
            customer,
            false,
        )
        .await?;
        let service=one(worker,"SELECT host_id FROM webdock_mail.service WHERE customer_id=$1 AND desired_enabled AND state='ready' AND revision=applied_revision AND EXISTS(SELECT 1 FROM webdock_auth.platform_settings WHERE id=true AND settings->>'mailEnabled'='true') FOR SHARE",vec![customer.into()]).await?.ok_or(Error::Unavailable)?;
        if field::<String>(&service, "host_id")? != self.managed_cluster {
            return Err(Error::Unavailable);
        }
        // Roles alone never grant content access, including platform operators. External grant
        // references are retained but cannot be reinterpreted as reviewed native membership.
        let row=one(worker,"SELECT b.id,b.name,b.provider_id AS account,d.name AS domain,b.local_part,i.id AS identity,g.revision FROM webdock_mail.mailbox b JOIN webdock_mail.domain d ON d.id=b.domain_id AND d.customer_id=b.customer_id JOIN webdock_mail.mailbox_grant g ON g.mailbox_id=b.id AND g.customer_id=b.customer_id AND g.subject=$3 JOIN webdock_mail.identity i ON i.customer_id=g.customer_id AND i.subject=g.subject WHERE b.id=$1 AND b.customer_id=$2 AND b.enabled AND g.enabled AND g.revision=g.applied_revision AND b.state='ready' AND i.state='ready' AND b.provider_id IS NOT NULL AND i.provider_id IS NOT NULL AND g.clerk_grant IS NULL AND (g.member_id=$4 OR (g.operator_grant AND $5)) FOR SHARE OF b,g,i",vec![mailbox.into(),customer.into(),subject.into(),member_id.into(),(operator || principal.is_acting_customer()).into()]).await?.ok_or(Error::Unavailable)?;
        let mailbox = Mailbox {
            id: field(&row, "id")?,
            name: field(&row, "name")?,
            account: field(&row, "account")?,
            domain: field(&row, "domain")?,
            local_part: field(&row, "local_part")?,
            identity: field(&row, "identity")?,
            revision: field(&row, "revision")?,
        };
        Ok(Authorized {
            auth,
            business,
            principal,
            mailbox,
        })
    }
    pub(crate) async fn finish(worker: DatabaseTransaction, authorized: Authorized) -> Result<()> {
        // Authority locks are held until the durable domain write has committed.
        worker.commit().await.map_err(|_| Error::Unavailable)?;
        authorized
            .business
            .rollback()
            .await
            .map_err(|_| Error::Unavailable)?;
        authorized
            .auth
            .rollback()
            .await
            .map_err(|_| Error::Unavailable)?;
        Ok(())
    }
    pub async fn launch(
        &self,
        principal: &Principal,
        authority: &StoredAuthority,
        customer: &str,
        mailbox: &str,
    ) -> Result<Launch> {
        let worker = self.worker().await?;
        let authorized = self
            .authorize(
                &worker,
                &principal.user_id,
                &principal.session_id,
                authority,
                customer,
                mailbox,
            )
            .await?;
        // Serialize launch count, not the whole platform, and bound opportunistic cleanup.
        one(
            &worker,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![format!("webmail-launch:{}", principal.session_id).into()],
        )
        .await?;
        run(&worker,"DELETE FROM webdock_mail.webmail_launch WHERE token_hash IN (SELECT token_hash FROM webdock_mail.webmail_launch WHERE expires_at<now() ORDER BY expires_at LIMIT 100)",vec![]).await?;
        run(&worker,"DELETE FROM webdock_mail.webmail_session WHERE id IN (SELECT id FROM webdock_mail.webmail_session WHERE expires_at<now() ORDER BY expires_at LIMIT 100)",vec![]).await?;
        let count:i64=field(&one(&worker,"SELECT count(*)::bigint AS count FROM webdock_mail.webmail_launch WHERE session_id=$1 AND consumed_at IS NULL",vec![principal.session_id.clone().into()]).await?.ok_or(Error::Unavailable)?,"count")?;
        if count >= 20 {
            return Err(Error::Unavailable);
        }
        let code = token()?;
        let expires =
            (Utc::now() + chrono::Duration::seconds(60)).min(authorized.principal.expires_at);
        run(&worker,"INSERT INTO webdock_mail.webmail_launch(token_hash,customer_id,mailbox_id,subject,session_id,grant_revision,expires_at,native_authority) VALUES($1,$2,$3,$4,$5,$6,$7,$8)",vec![hash(&code)?.into(),customer.into(),mailbox.into(),principal.user_id.clone().into(),principal.session_id.clone().into(),authorized.mailbox.revision.into(),expires.into(),serde_json::to_value(authority).map_err(|_|Error::Invalid)?.into()]).await?;
        Self::finish(worker, authorized).await?;
        Ok(Launch {
            code,
            gateway_origin: self.gateway_origin.clone(),
            mailbox_id: mailbox.into(),
            customer_id: customer.into(),
            expires_at: expires,
        })
    }
    pub async fn exchange(&self, code: &str) -> Result<Exchanged> {
        let hash = hash(code)?;
        let id = self
            .dependencies
            .ids
            .allocate()
            .await
            .map_err(|_| Error::Unavailable)?
            .to_string();
        let worker = self.worker().await?;
        let row=one(&worker,"UPDATE webdock_mail.webmail_launch SET consumed_at=now() WHERE token_hash=$1 AND consumed_at IS NULL AND expires_at>now() RETURNING *",vec![hash.into()]).await?.ok_or(Error::Unavailable)?;
        let bound = Bound::read(&row, false)?;
        let authorized = self
            .authorize(
                &worker,
                &bound.subject,
                &bound.session,
                &bound.authority,
                &bound.customer,
                &bound.mailbox,
            )
            .await?;
        if bound.revision != authorized.mailbox.revision {
            return Err(Error::Unavailable);
        }
        let token = token()?;
        let expires =
            (Utc::now() + chrono::Duration::minutes(15)).min(authorized.principal.expires_at);
        run(&worker,"INSERT INTO webdock_mail.webmail_session(id,token_hash,customer_id,mailbox_id,subject,session_id,grant_revision,expires_at,native_authority) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",vec![id.clone().into(),crate::webmail::hash(&token)?.into(),bound.customer.clone().into(),bound.mailbox.clone().into(),bound.subject.into(),bound.session.into(),bound.revision.into(),expires.into(),serde_json::to_value(bound.authority).map_err(|_|Error::Invalid)?.into()]).await?;
        Self::finish(worker, authorized).await?;
        Ok(Exchanged {
            token,
            scope_id: id,
            customer_id: bound.customer,
            mailbox_id: bound.mailbox,
            expires_at: expires,
        })
    }
    pub(crate) async fn inspect_in(
        &self,
        worker: &DatabaseTransaction,
        value: &str,
    ) -> Result<(GatewayAccess, Authorized)> {
        let row=one(worker,"SELECT * FROM webdock_mail.webmail_session WHERE token_hash=$1 AND expires_at>now() FOR SHARE",vec![hash(value)?.into()]).await?.ok_or(Error::Unavailable)?;
        let bound = Bound::read(&row, true)?;
        let authorized = self
            .authorize(
                worker,
                &bound.subject,
                &bound.session,
                &bound.authority,
                &bound.customer,
                &bound.mailbox,
            )
            .await?;
        if bound.revision != authorized.mailbox.revision {
            return Err(Error::Unavailable);
        }
        let mailbox = &authorized.mailbox;
        let row=one(worker,"SELECT encrypted_credentials FROM webdock_mail.identity WHERE id=$1 AND customer_id=$2",vec![mailbox.identity.clone().into(),bound.customer.clone().into()]).await?.ok_or(Error::Unavailable)?;
        #[derive(Deserialize)]
        struct Credential {
            username: String,
            password: String,
        }
        let secret: Credential = self.cipher.open(
            &field::<Option<String>>(&row, "encrypted_credentials")?.ok_or(Error::Unavailable)?,
            &format!("identity:{}:{}", bound.customer, mailbox.identity),
        )?;
        if secret.username.is_empty()
            || secret.username.contains([':', '\r', '\n', '\0'])
            || secret.password.is_empty()
            || secret.password.contains(['\r', '\n', '\0'])
        {
            return Err(Error::Unavailable);
        }
        let access = GatewayAccess {
            scope_id: bound.id.ok_or(Error::Unavailable)?,
            customer_id: bound.customer.clone(),
            mailbox_id: mailbox.id.clone(),
            account_id: mailbox.account.clone(),
            sender_name: mailbox.name.clone(),
            sender_address: format!("{}@{}", mailbox.local_part, mailbox.domain),
            runtime_origin: format!(
                "http://app-{}.wd-mail-{}.svc.cluster.local:8080",
                bound.customer, bound.customer
            ),
            authorization: format!(
                "Basic {}",
                STANDARD.encode(format!("{}:{}", secret.username, secret.password))
            ),
            subject: bound.subject,
            grant_revision: mailbox.revision.to_string(),
            expires_at: bound.expires.min(authorized.principal.expires_at),
        };
        Ok((access, authorized))
    }
    pub async fn inspect(&self, value: &str) -> Result<GatewayAccess> {
        let worker = self.worker().await?;
        let (access, authorized) = self.inspect_in(&worker, value).await?;
        Self::finish(worker, authorized).await?;
        Ok(access)
    }
    pub async fn revoke(&self, value: &str) -> Result<()> {
        let worker = self.worker().await?;
        run(
            &worker,
            "DELETE FROM webdock_mail.webmail_session WHERE token_hash=$1",
            vec![hash(value)?.into()],
        )
        .await?;
        worker.commit().await.map_err(|_| Error::Unavailable)
    }
    pub async fn reserve_write(&self, command: ReserveWrite) -> Result<WriteReservation> {
        if command.payload_hash.len() != 64
            || !command
                .payload_hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || command.resource_id.as_ref().is_some_and(|v| !resource(v))
            || (!matches!(command.action, WriteAction::Draft) && command.resource_id.is_none())
        {
            return Err(Error::Invalid);
        }
        let id = self
            .dependencies
            .ids
            .allocate()
            .await
            .map_err(|_| Error::Unavailable)?
            .to_string();
        let worker = self.worker().await?;
        let (access, authorized) = self.inspect_in(&worker, &command.token).await?;
        self.module(&authorized.business, true).await?;
        one(
            &worker,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![format!("webmail-write:{}", access.mailbox_id).into()],
        )
        .await?;
        let existing=one(&worker,"SELECT id,action,resource_id,payload_hash,state,result FROM webdock_mail.webmail_write WHERE mailbox_id=$1 AND ((subject=$2 AND request_id=$3::uuid) OR (action='submit' AND $4='submit' AND resource_id=$5 AND state<>'rejected')) ORDER BY created_at LIMIT 1",vec![access.mailbox_id.clone().into(),access.subject.clone().into(),command.request_id.to_string().into(),command.action.as_str().into(),command.resource_id.clone().into()]).await?;
        if let Some(row) = existing {
            if field::<String>(&row, "action")? != command.action.as_str()
                || field::<Option<String>>(&row, "resource_id")? != command.resource_id
                || field::<String>(&row, "payload_hash")? != command.payload_hash
            {
                return Err(Error::Unavailable);
            }
            let result = WriteReservation {
                execute: false,
                operation_id: field(&row, "id")?,
                lease_token: None,
                state: field(&row, "state")?,
                result: field(&row, "result")?,
            };
            Self::finish(worker, authorized).await?;
            return Ok(result);
        }
        if let Some(resource) = &command.resource_id
            && matches!(command.action, WriteAction::Draft | WriteAction::Submit)
        {
            let opposite = if matches!(command.action, WriteAction::Draft) {
                "submit"
            } else {
                "draft"
            };
            if one(&worker,"SELECT id FROM webdock_mail.webmail_write WHERE mailbox_id=$1 AND resource_id=$2 AND action=$3 AND state<>'rejected' LIMIT 1",vec![access.mailbox_id.clone().into(),resource.clone().into(),opposite.into()]).await?.is_some(){return Err(Error::Unavailable);}
        }
        let count:i64=field(&one(&worker,"SELECT count(*)::bigint AS count FROM webdock_mail.webmail_write WHERE mailbox_id=$1 AND created_at>now()-interval '1 hour'",vec![access.mailbox_id.clone().into()]).await?.ok_or(Error::Unavailable)?,"count")?;
        if count >= 1000 {
            return Err(Error::Unavailable);
        }
        let lease = uuid::Uuid::new_v4();
        run(&worker,"INSERT INTO webdock_mail.webmail_write(id,customer_id,mailbox_id,subject,request_id,action,resource_id,payload_hash,lease_token) VALUES($1,$2,$3,$4,$5::uuid,$6,$7,$8,$9::uuid)",vec![id.clone().into(),access.customer_id.into(),access.mailbox_id.into(),access.subject.into(),command.request_id.to_string().into(),command.action.as_str().into(),command.resource_id.into(),command.payload_hash.into(),lease.to_string().into()]).await?;
        Self::finish(worker, authorized).await?;
        Ok(WriteReservation {
            execute: true,
            operation_id: id,
            lease_token: Some(lease),
            state: "reserved".into(),
            result: None,
        })
    }
    pub async fn finish_write(&self, command: FinishWrite) -> Result<()> {
        if !valid_id(&command.operation_id)
            || (!matches!(command.state, WriteState::Succeeded) && command.result.is_some())
            || command.result.as_ref().is_some_and(|r| {
                r.id.as_ref().is_some_and(|s| !resource(s))
                    || r.state
                        .as_ref()
                        .is_some_and(|s| s.encode_utf16().count() > 255)
            })
        {
            return Err(Error::Invalid);
        }
        let result = command
            .result
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| Error::Invalid)?;
        let worker = self.worker().await?;
        let (access, authorized) = self.inspect_in(&worker, &command.token).await?;
        let row=one(&worker,"SELECT state,result FROM webdock_mail.webmail_write WHERE id=$1 AND lease_token=$2::uuid AND customer_id=$3 AND mailbox_id=$4 AND subject=$5 FOR UPDATE",vec![command.operation_id.clone().into(),command.lease_token.to_string().into(),access.customer_id.into(),access.mailbox_id.into(),access.subject.into()]).await?.ok_or(Error::Unavailable)?;
        let state: String = field(&row, "state")?;
        if state != "reserved" {
            if state != command.state.as_str() || field::<Option<Value>>(&row, "result")? != result
            {
                return Err(Error::Unavailable);
            }
        } else {
            run(&worker,"UPDATE webdock_mail.webmail_write SET state=$2,result=$3,finished_at=now() WHERE id=$1",vec![command.operation_id.into(),command.state.as_str().into(),result.into()]).await?;
        }
        Self::finish(worker, authorized).await
    }
}
