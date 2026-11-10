//! Recovery domain SQL; canonical locks protect proof admission and consumption.
use super::{DeliveryBatch, DeliveryKind, DeliveryRequest};
use crate::{
    IdentityError, IdentityService, Result,
    crypto::{hash_password, random},
    security::digest,
};
use spitzli_persistence::postgres::{Connection, Pool, Transaction};
use std::time::Duration;
use tokio_postgres::types::Type;
const BUDGET: Duration = Duration::from_secs(30);

// The caller holds the canonical user FOR UPDATE: a new mapping FK cannot race admission.
// Binding metadata is SELECT-only for bootstrap; do not widen privileges for a row lock.
async fn managed(tx: &Transaction<'_>, user: &str) -> Result<bool> {
    Ok(tx
        .query_typed_opt(
            "SELECT user_id FROM webdock_auth.neon_identity WHERE user_id=$1",
            &[(&user, Type::TEXT)],
        )
        .await
        .map_err(|_| IdentityError::Persistence)?
        .is_some())
}
async fn local(tx: &Transaction<'_>, user: &str) -> Result<bool> {
    tx.query_typed_one(
        "SELECT webdock_rls.local_identity_authority($1) AS allowed",
        &[(&user, Type::TEXT)],
    )
    .await
    .map_err(|_| IdentityError::Persistence)?
    .try_get("allowed")
    .map_err(|_| IdentityError::Persistence)
}
async fn credential(tx: &Transaction<'_>, user: &str) -> Result<String> {
    let rows=tx.query_typed(r#"SELECT password FROM webdock_auth.account WHERE "userId"=$1 AND "providerId"='credential' FOR UPDATE"#,&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
    if rows.len() != 1 {
        return Err(IdentityError::Unauthorized);
    }
    rows[0]
        .try_get::<_, Option<String>>("password")
        .map_err(|_| IdentityError::Persistence)?
        .ok_or(IdentityError::Unauthorized)
}
impl IdentityService {
    pub fn with_native_account_recovery(mut self, pool: Pool) -> Self {
        self.native_recovery = Some(pool);
        self
    }
    async fn recovery_connection(&self) -> Result<Connection> {
        self.native_recovery
            .as_ref()
            .ok_or(IdentityError::Configuration)?
            .acquire()
            .await
            .map_err(|_| IdentityError::Persistence)
    }
    pub(crate) async fn native_request_password_reset(&self, email: &str) -> Result<()> {
        let provider=tokio::time::timeout(BUDGET,async {
            let mut connection=self.recovery_connection().await?;
            let tx=connection.transaction().await.map_err(|_|IdentityError::Persistence)?;
            let rows=tx.query_typed(r#"SELECT id,"mustChangePassword","emailVerified",coalesce(banned,false) AS banned FROM webdock_auth."user" WHERE lower(email)=$1 LIMIT 2 FOR UPDATE"#,&[(&email,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
            let mut provider=false;
            if rows.len()==1 {
                let row=&rows[0];let id:String=row.try_get("id").map_err(|_|IdentityError::Persistence)?;
                provider=managed(&tx,&id).await?;
                if !provider && !row.try_get::<_,bool>("banned").map_err(|_|IdentityError::Persistence)? && local(&tx,&id).await? {
                    let kind=if row.try_get::<_,bool>("mustChangePassword").map_err(|_|IdentityError::Persistence)? && !row.try_get::<_,bool>("emailVerified").map_err(|_|IdentityError::Persistence)? {DeliveryKind::Activation}else{DeliveryKind::Reset};
                    self.queue_native_recovery(&tx,&id,kind,None).await?;
                }
            }
            tx.commit().await.map_err(|_|IdentityError::Persistence)?;
            Ok::<_,IdentityError>(provider)
        }).await.map_err(|_|IdentityError::Persistence)??;
        // Provider I/O holds no database connection. A mapped account never falls back locally.
        if provider {
            let redirect = self
                .account_origin
                .as_ref()
                .ok_or(IdentityError::Configuration)?
                .join("/reset-password")
                .map_err(|_| IdentityError::Configuration)?;
            self.neon
                .as_ref()
                .ok_or(IdentityError::Configuration)?
                .request_reset(email, redirect.as_str())
                .await?;
        }
        Ok(())
    }
    pub(crate) async fn native_queue_account_activation(
        &self,
        user: &str,
        invitation: Option<&str>,
    ) -> Result<Option<String>> {
        tokio::time::timeout(BUDGET,async {
            let mut connection=self.recovery_connection().await?;
            let tx=connection.transaction().await.map_err(|_|IdentityError::Persistence)?;
            let row=tx.query_typed_opt(r#"SELECT id FROM webdock_auth."user" WHERE id=$1 AND NOT "emailVerified" AND "mustChangePassword" AND NOT coalesce(banned,false) FOR UPDATE"#,&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
            if row.is_none() || !local(&tx,user).await? || managed(&tx,user).await? {return Err(IdentityError::SecurityState);}
            let link=self.queue_native_recovery(&tx,user,DeliveryKind::Activation,invitation).await?;
            tx.commit().await.map_err(|_|IdentityError::Persistence)?;Ok(link)
        }).await.map_err(|_|IdentityError::Persistence)?
    }
    async fn queue_native_recovery(
        &self,
        tx: &Transaction<'_>,
        user: &str,
        kind: DeliveryKind,
        invitation: Option<&str>,
    ) -> Result<Option<String>> {
        let keys = self.keys.as_ref().ok_or(IdentityError::Configuration)?;
        let origin = self
            .account_origin
            .as_ref()
            .ok_or(IdentityError::Configuration)?;
        let allocator = self
            .allocator
            .as_ref()
            .ok_or(IdentityError::Configuration)?;
        tx.execute_typed("DELETE FROM webdock_auth.identity_recovery_token WHERE user_id=$1 AND expires_at<=statement_timestamp()",&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
        if tx.query_typed_opt("SELECT token_hash FROM webdock_auth.identity_recovery_token WHERE user_id=$1 AND created_at>statement_timestamp()-interval '1 minute' LIMIT 1",&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?.is_some(){return Ok(None);}
        let email: String = tx
            .query_typed_one(
                r#"SELECT email FROM webdock_auth."user" WHERE id=$1"#,
                &[(&user, Type::TEXT)],
            )
            .await
            .map_err(|_| IdentityError::Persistence)?
            .try_get("email")
            .map_err(|_| IdentityError::Persistence)?;
        let fingerprint = digest(&credential(tx, user).await?);
        if !local(tx, user).await? || managed(tx, user).await? {
            return Err(IdentityError::Unauthorized);
        }
        let token = hex::encode(random::<32>()?);
        let hash = digest(&token);
        tx.execute_typed("INSERT INTO webdock_auth.identity_recovery_token(token_hash,user_id,email,credential_digest,kind,expires_at) VALUES($1,$2,$3,$4,$5,statement_timestamp()+interval '1 hour')",&[(&hash,Type::TEXT),(&user,Type::TEXT),(&email,Type::TEXT),(&fingerprint,Type::TEXT),(&kind.as_str(),Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
        let id = allocator
            .allocate()
            .await
            .map_err(|_| IdentityError::Allocation)?
            .to_string();
        let mut url = origin
            .join("/reset-password")
            .map_err(|_| IdentityError::Configuration)?;
        url.query_pairs_mut().append_pair("token", &token);
        if let Some(invitation) = invitation {
            url.query_pairs_mut().append_pair("invitation", invitation);
        }
        let link = String::from(url);
        let payload = DeliveryRequest {
            id: id.clone(),
            email,
            kind,
            url: link.clone(),
        };
        let encrypted = keys.encrypt(
            &serde_json::to_string(&payload).map_err(|_| IdentityError::InvalidEncoding)?,
        )?;
        tx.execute_typed("INSERT INTO webdock_auth.identity_delivery_job(id,token_hash,payload) VALUES($1,$2,$3)",&[(&id,Type::TEXT),(&hash,Type::TEXT),(&encrypted,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
        Ok(Some(link))
    }
    pub(crate) async fn native_dispatch_delivery_jobs(&self, limit: u32) -> Result<DeliveryBatch> {
        let keys = self.keys.as_ref().ok_or(IdentityError::Configuration)?;
        let mut result = DeliveryBatch::default();
        for _ in 0..limit {
            let job=tokio::time::timeout(BUDGET,async {
                let mut connection=self.recovery_connection().await?;let tx=connection.transaction().await.map_err(|_|IdentityError::Persistence)?;
                let job=tx.query_typed_opt("UPDATE webdock_auth.identity_delivery_job d SET attempts=d.attempts+1,lease_until=statement_timestamp()+interval '60 seconds' WHERE d.id=(SELECT j.id FROM webdock_auth.identity_delivery_job j JOIN webdock_auth.identity_recovery_token t ON t.token_hash=j.token_hash WHERE j.delivered_at IS NULL AND j.attempts<5 AND j.next_attempt_at<=statement_timestamp() AND (j.lease_until IS NULL OR j.lease_until<=statement_timestamp()) AND t.expires_at>statement_timestamp() ORDER BY j.created_at,j.id FOR UPDATE OF j SKIP LOCKED LIMIT 1) RETURNING id,payload,attempts",&[]).await.map_err(|_|IdentityError::Persistence)?;
                tx.commit().await.map_err(|_|IdentityError::Persistence)?;Ok::<_,IdentityError>(job)
            }).await.map_err(|_|IdentityError::Persistence)??;
            let Some(job) = job else {
                break;
            };
            let id: String = job.try_get("id").map_err(|_| IdentityError::Persistence)?;
            let generation: i32 = job
                .try_get("attempts")
                .map_err(|_| IdentityError::Persistence)?;
            let request: DeliveryRequest = serde_json::from_str(
                &keys.decrypt(
                    &job.try_get::<_, String>("payload")
                        .map_err(|_| IdentityError::Persistence)?,
                )?,
            )
            .map_err(|_| IdentityError::InvalidEncoding)?;
            if request.id != id {
                return Err(IdentityError::InvalidEncoding);
            }
            let delivered = matches!(
                tokio::time::timeout(Duration::from_secs(10), self.delivery.deliver(&request))
                    .await,
                Ok(Ok(()))
            );
            tokio::time::timeout(BUDGET,async {
                let mut connection=self.recovery_connection().await?;let tx=connection.transaction().await.map_err(|_|IdentityError::Persistence)?;
                let changed=tx.execute_typed("UPDATE webdock_auth.identity_delivery_job SET delivered_at=CASE WHEN $3 THEN statement_timestamp() ELSE NULL END,lease_until=NULL,next_attempt_at=statement_timestamp()+make_interval(secs=>30*attempts) WHERE id=$1 AND attempts=$2",&[(&id,Type::TEXT),(&generation,Type::INT4),(&delivered,Type::BOOL)]).await.map_err(|_|IdentityError::Persistence)?;
                if changed!=1 {return Err(IdentityError::Persistence);}
                tx.commit().await.map_err(|_|IdentityError::Persistence)
            }).await.map_err(|_|IdentityError::Persistence)??;
            if delivered {
                result.delivered += 1;
            } else {
                result.failed += 1;
            }
        }
        Ok(result)
    }
    pub(crate) async fn native_complete_password_reset(
        &self,
        token: &str,
        password: &str,
    ) -> Result<String> {
        tokio::time::timeout(BUDGET,async {
            let hash=digest(token);let mut connection=self.recovery_connection().await?;let tx=connection.transaction().await.map_err(|_|IdentityError::Persistence)?;
            let lookup=tx.query_typed_opt("SELECT user_id FROM webdock_auth.identity_recovery_token WHERE token_hash=$1",&[(&hash,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?.ok_or(IdentityError::Unauthorized)?;
            let user:String=lookup.try_get("user_id").map_err(|_|IdentityError::Persistence)?;
            let row=tx.query_typed_opt(r#"SELECT email FROM webdock_auth."user" WHERE id=$1 AND NOT coalesce(banned,false) FOR UPDATE"#,&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?.ok_or(IdentityError::Unauthorized)?;
            if !local(&tx,&user).await? || managed(&tx,&user).await? {return Err(IdentityError::Unauthorized);}
            let email:String=row.try_get("email").map_err(|_|IdentityError::Persistence)?;
            let proof=tx.query_typed_opt("SELECT email,credential_digest FROM webdock_auth.identity_recovery_token WHERE token_hash=$1 AND user_id=$2 AND expires_at>statement_timestamp() FOR UPDATE",&[(&hash,Type::TEXT),(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?.ok_or(IdentityError::Unauthorized)?;
            if proof.try_get::<_,String>("email").map_err(|_|IdentityError::Persistence)?!=email || proof.try_get::<_,String>("credential_digest").map_err(|_|IdentityError::Persistence)?!=digest(&credential(&tx,&user).await?) {return Err(IdentityError::Unauthorized);}
            let password=password.to_owned();let permit=self.password_workers.clone().acquire_owned().await.map_err(|_|IdentityError::Worker)?;
            let encoded=tokio::task::spawn_blocking(move ||{let _permit=permit;hash_password(&password)}).await.map_err(|_|IdentityError::Worker)??;
            // Refresh migration/provider authority after credential waits and expensive hashing.
            if !local(&tx,&user).await? || managed(&tx,&user).await? {return Err(IdentityError::Unauthorized);}
            // A fresh statement checks expiry after all lock waits and password hashing.
            if tx.execute_typed("DELETE FROM webdock_auth.identity_recovery_token WHERE token_hash=$1 AND user_id=$2 AND expires_at>statement_timestamp()",&[(&hash,Type::TEXT),(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?!=1{return Err(IdentityError::Unauthorized);}
            if tx.execute_typed(r#"UPDATE webdock_auth.account SET password=$1,"updatedAt"=statement_timestamp() WHERE "userId"=$2 AND "providerId"='credential'"#,&[(&encoded,Type::TEXT),(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?!=1 {return Err(IdentityError::Unauthorized);}
            tx.execute_typed(r#"UPDATE webdock_auth."user" SET "mustChangePassword"=false,"emailVerified"=true,"updatedAt"=statement_timestamp() WHERE id=$1"#,&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
            tx.execute_typed("DELETE FROM webdock_auth.identity_mfa_challenge WHERE user_id=$1",&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
            tx.execute_typed(r#"DELETE FROM webdock_auth.session WHERE "userId"=$1"#,&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
            tx.execute_typed("DELETE FROM webdock_auth.identity_recovery_token WHERE user_id=$1",&[(&user,Type::TEXT)]).await.map_err(|_|IdentityError::Persistence)?;
            tx.commit().await.map_err(|_|IdentityError::Persistence)?;Ok(email)
        }).await.map_err(|_|IdentityError::Persistence)?
    }
}
