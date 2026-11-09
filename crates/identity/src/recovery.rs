use crate::{
    IdentityError, IdentityService, Result,
    crypto::{hash_password, random},
    security::{credential, digest, invalidate_sessions},
    service::externally_managed,
};
use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
#[path = "native_recovery.rs"]
mod native_recovery;
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DeliveryKind {
    Reset,
    Activation,
    Invitation,
    WebsiteAccess,
    /// Something in a workspace needs attention; the details are in the console.
    Notice,
}
impl DeliveryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reset => "reset",
            Self::Activation => "activation",
            Self::Invitation => "invitation",
            Self::WebsiteAccess => "website-access",
            Self::Notice => "notice",
        }
    }
}
/// Contains a one-use recovery URL. Never log or return this job payload to public callers.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryRequest {
    pub id: String,
    pub email: String,
    pub kind: DeliveryKind,
    pub url: String,
}
#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    #[error("account delivery is unavailable")]
    Unavailable,
}
#[async_trait]
pub trait Delivery: Send + Sync {
    async fn deliver(&self, request: &DeliveryRequest) -> std::result::Result<(), DeliveryError>;
}
pub struct UnavailableDelivery;
#[async_trait]
impl Delivery for UnavailableDelivery {
    async fn deliver(&self, _: &DeliveryRequest) -> std::result::Result<(), DeliveryError> {
        Err(DeliveryError::Unavailable)
    }
}
#[derive(Debug, Default)]
pub struct DeliveryBatch {
    pub delivered: u32,
    pub failed: u32,
}
pub fn validate_account_origin(origin: &url::Url) -> Result<()> {
    let local = origin.scheme() == "http"
        && [Some("localhost"), Some("127.0.0.1"), Some("[::1]")].contains(&origin.host_str());
    if (!local && origin.scheme() != "https")
        || origin.host_str().is_none()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return Err(IdentityError::Configuration);
    }
    Ok(())
}
impl IdentityService {
    pub fn with_account_origin(mut self, origin: url::Url) -> Result<Self> {
        validate_account_origin(&origin)?;
        self.account_origin = Some(origin);
        Ok(self)
    }
    pub fn with_delivery(mut self, delivery: Arc<dyn Delivery>) -> Self {
        self.delivery = delivery;
        self
    }
    /// Always return the same HTTP acknowledgement, including unknown/disabled accounts.
    pub async fn request_password_reset(&self, email: &str) -> Result<()> {
        self.account_origin
            .as_ref()
            .ok_or(IdentityError::Configuration)?;
        self.keys.as_ref().ok_or(IdentityError::Configuration)?;
        if email.len() > 254 {
            return Ok(());
        }
        let email = email.trim().to_lowercase();
        if self.native_recovery.is_some() {
            return self.native_request_password_reset(&email).await;
        }
        if self.neon_mapping_email(&email).await?.is_some() {
            let redirect = self
                .account_origin
                .as_ref()
                .ok_or(IdentityError::Configuration)?
                .join("/reset-password")
                .map_err(|_| IdentityError::Configuration)?;
            self.neon
                .as_ref()
                .ok_or(IdentityError::Configuration)?
                .request_reset(&email, redirect.as_str())
                .await?;
            return Ok(());
        }
        let tx = self.db.begin().await?;
        let rows=tx.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,\"mustChangePassword\",\"emailVerified\" FROM webdock_auth.\"user\" WHERE lower(email)=$1 AND NOT coalesce(banned,false) FOR UPDATE",[email.into()])).await?;
        if rows.len() == 1 {
            let id: String = rows[0].try_get("", "id")?;
            if !externally_managed(&tx, &id).await? {
                let kind = if rows[0].try_get::<bool>("", "mustChangePassword")?
                    && !rows[0].try_get::<bool>("", "emailVerified")?
                {
                    DeliveryKind::Activation
                } else {
                    DeliveryKind::Reset
                };
                self.queue_recovery(&tx, &id, kind, None).await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
    /// Operator/invitation service only. Binds activation to an existing canonical identity.
    /// Returns the one-use link, or None when one was issued within the last minute.
    /// `invitation` lets the activation page continue straight into that invitation.
    pub async fn queue_account_activation(
        &self,
        user_id: &str,
        invitation: Option<&str>,
    ) -> Result<Option<String>> {
        if self.native_recovery.is_some() {
            return self
                .native_queue_account_activation(user_id, invitation)
                .await;
        }
        let tx = self.db.begin().await?;
        if tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.\"user\" WHERE id=$1 AND NOT \"emailVerified\" AND \"mustChangePassword\" AND NOT coalesce(banned,false) FOR UPDATE",[user_id.into()])).await?.is_none()||externally_managed(&tx,user_id).await? {return Err(IdentityError::SecurityState)}
        let link = self
            .queue_recovery(&tx, user_id, DeliveryKind::Activation, invitation)
            .await?;
        tx.commit().await?;
        Ok(link)
    }
    async fn queue_recovery(
        &self,
        tx: &DatabaseTransaction,
        id: &str,
        kind: DeliveryKind,
        invitation: Option<&str>,
    ) -> Result<Option<String>> {
        let keys = self.keys.as_ref().ok_or(IdentityError::Configuration)?;
        let origin = self
            .account_origin
            .as_ref()
            .ok_or(IdentityError::Configuration)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"DELETE FROM webdock_auth.identity_recovery_token WHERE user_id=$1 AND expires_at<=statement_timestamp()",[id.into()])).await?;
        // One active request per minute per account prevents repeated emails without exposing existence.
        if tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT token_hash FROM webdock_auth.identity_recovery_token WHERE user_id=$1 AND created_at>statement_timestamp()-interval '1 minute' LIMIT 1",[id.into()])).await?.is_some(){return Ok(None)}
        let email: String = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT email FROM webdock_auth.\"user\" WHERE id=$1",
                [id.into()],
            ))
            .await?
            .ok_or(IdentityError::Unauthorized)?
            .try_get("", "email")?;
        let fingerprint = digest(&credential(tx, id).await?);
        let token = hex::encode(random::<32>()?);
        let token_hash = digest(&token);
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.identity_recovery_token(token_hash,user_id,email,credential_digest,kind,expires_at) VALUES($1,$2,$3,$4,$5,statement_timestamp()+interval '1 hour')",[token_hash.clone().into(),id.into(),email.clone().into(),fingerprint.into(),kind.as_str().into()])).await?;
        let job_id = self.allocate_id(tx).await?;
        let mut url = origin
            .join("/reset-password")
            .map_err(|_| IdentityError::Configuration)?;
        url.query_pairs_mut().append_pair("token", &token);
        if let Some(invitation) = invitation {
            url.query_pairs_mut().append_pair("invitation", invitation);
        }
        let link = String::from(url);
        let payload = DeliveryRequest {
            id: job_id.clone(),
            email,
            kind,
            url: link.clone(),
        };
        let encrypted = keys.encrypt(
            &serde_json::to_string(&payload).map_err(|_| IdentityError::InvalidEncoding)?,
        )?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.identity_delivery_job(id,token_hash,payload) VALUES($1,$2,$3)",[job_id.into(),token_hash.into(),encrypted.into()])).await?;
        Ok(Some(link))
    }
    /// Worker call. The delivery sink must deduplicate on request.id; delivery is at-least-once.
    pub async fn dispatch_delivery_jobs(&self, limit: u32) -> Result<DeliveryBatch> {
        if limit == 0 || limit > 100 {
            return Err(IdentityError::InvalidInput);
        }
        if self.native_recovery.is_some() {
            return self.native_dispatch_delivery_jobs(limit).await;
        }
        let keys = self.keys.as_ref().ok_or(IdentityError::Configuration)?;
        let mut result = DeliveryBatch::default();
        for _ in 0..limit {
            let job=self.db.query_one_raw(Statement::from_string(DbBackend::Postgres,"UPDATE webdock_auth.identity_delivery_job d SET attempts=d.attempts+1,lease_until=statement_timestamp()+interval '60 seconds' WHERE d.id=(SELECT j.id FROM webdock_auth.identity_delivery_job j JOIN webdock_auth.identity_recovery_token t ON t.token_hash=j.token_hash WHERE j.delivered_at IS NULL AND j.attempts<5 AND j.next_attempt_at<=statement_timestamp() AND (j.lease_until IS NULL OR j.lease_until<=statement_timestamp()) AND t.expires_at>statement_timestamp() ORDER BY j.created_at,j.id FOR UPDATE OF j SKIP LOCKED LIMIT 1) RETURNING id,payload,attempts")).await?;
            let Some(job) = job else { break };
            let id: String = job.try_get("", "id")?;
            let generation: i32 = job.try_get("", "attempts")?;
            let request: DeliveryRequest =
                serde_json::from_str(&keys.decrypt(&job.try_get::<String>("", "payload")?)?)
                    .map_err(|_| IdentityError::InvalidEncoding)?;
            if request.id != id {
                return Err(IdentityError::InvalidEncoding);
            }
            let delivered = matches!(
                tokio::time::timeout(Duration::from_secs(10), self.delivery.deliver(&request))
                    .await,
                Ok(Ok(()))
            );
            self.db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.identity_delivery_job SET delivered_at=CASE WHEN $3 THEN statement_timestamp() ELSE NULL END,lease_until=NULL,next_attempt_at=statement_timestamp()+make_interval(secs=>30*attempts) WHERE id=$1 AND attempts=$2",[id.into(),generation.into(),delivered.into()])).await?;
            if delivered {
                result.delivered += 1
            } else {
                result.failed += 1
            }
        }
        Ok(result)
    }
    /// Returns the account email so the holder of the mailed link can sign in next.
    pub async fn complete_password_reset(&self, token: &str, new_password: &str) -> Result<String> {
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(IdentityError::Unauthorized);
        }
        if self.native_recovery.is_some() {
            return self
                .native_complete_password_reset(token, new_password)
                .await;
        }
        let hash_token = digest(token);
        let tx = self.db.begin().await?;
        let lookup = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT user_id FROM webdock_auth.identity_recovery_token WHERE token_hash=$1",
                [hash_token.clone().into()],
            ))
            .await?
            .ok_or(IdentityError::Unauthorized)?;
        let id: String = lookup.try_get("", "user_id")?;
        let user=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT email FROM webdock_auth.\"user\" WHERE id=$1 AND NOT coalesce(banned,false) FOR UPDATE",[id.clone().into()])).await?.ok_or(IdentityError::Unauthorized)?;
        if externally_managed(&tx, &id).await? || self.neon_mapping(&tx, &id).await?.is_some() {
            return Err(IdentityError::Unauthorized);
        }
        let recovery=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT email,credential_digest FROM webdock_auth.identity_recovery_token WHERE token_hash=$1 AND user_id=$2 AND expires_at>statement_timestamp() FOR UPDATE",[hash_token.into(),id.clone().into()])).await?.ok_or(IdentityError::Unauthorized)?;
        if recovery.try_get::<String>("", "email")? != user.try_get::<String>("", "email")?
            || recovery.try_get::<String>("", "credential_digest")?
                != digest(&credential(&tx, &id).await?)
        {
            return Err(IdentityError::Unauthorized);
        }
        let new_password = new_password.to_owned();
        let permit = self
            .password_workers
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| IdentityError::Worker)?;
        let hash = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            hash_password(&new_password)
        })
        .await
        .map_err(|_| IdentityError::Worker)??;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.account SET password=$1,\"updatedAt\"=statement_timestamp() WHERE \"userId\"=$2 AND \"providerId\"='credential'",[hash.into(),id.clone().into()])).await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"user\" SET \"mustChangePassword\"=false,\"emailVerified\"=true,\"updatedAt\"=statement_timestamp() WHERE id=$1",[id.clone().into()])).await?;
        invalidate_sessions(&tx, &id, None).await?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.identity_recovery_token WHERE user_id=$1",
            [id.into()],
        ))
        .await?;
        tx.commit().await?;
        Ok(user.try_get("", "email")?)
    }
}
