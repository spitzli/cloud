use crate::{IdentityError, Principal, Result, crypto::mac};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait,
};
use serde_json::json;

/// Signs only verified session identities; SQL independently revalidates session and role.
pub struct RlsSigner {
    secret: String,
    audience: String,
}
impl RlsSigner {
    pub fn new(secret: impl Into<String>, audience: impl Into<String>) -> Result<Self> {
        let secret = secret.into();
        let audience = audience.into();
        if secret.len() < 32
            || !["webdock_auth_runtime", "webdock_registry_runtime"].contains(&audience.as_str())
        {
            return Err(IdentityError::Configuration);
        }
        Ok(Self { secret, audience })
    }
    pub async fn begin(
        &self,
        db: &DatabaseConnection,
        principal: &Principal,
    ) -> Result<DatabaseTransaction> {
        let tx = db.begin().await?;
        self.apply(&tx, principal).await?;
        Ok(tx)
    }
    pub async fn apply(&self, tx: &DatabaseTransaction, principal: &Principal) -> Result<()> {
        let (raw, signature) = self.signed_scope(principal)?;
        tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)", [raw.into(), signature.into()])).await?;
        Ok(())
    }
    /// Own the transaction while installing authority: failure/cancellation cannot leave
    /// an executable transaction with uncertain scope in the caller's hands.
    pub async fn apply_native<'a>(
        &self,
        tx: spitzli_persistence::postgres::Transaction<'a>,
        principal: &Principal,
    ) -> Result<spitzli_persistence::postgres::Transaction<'a>> {
        let (raw, signature) = self.signed_scope(principal)?;
        tokio::time::timeout(std::time::Duration::from_secs(5), tx.query_typed_opt("SELECT pg_catalog.set_config('webdock.rls_context',$1,true),pg_catalog.set_config('webdock.rls_signature',$2,true) WHERE current_user::text=$3", &[(&raw,tokio_postgres::types::Type::TEXT),(&signature,tokio_postgres::types::Type::TEXT),(&self.audience,tokio_postgres::types::Type::TEXT)])).await.map_err(|_|IdentityError::Persistence)?.map_err(|_|IdentityError::Persistence)?.ok_or(IdentityError::Persistence)?;
        Ok(tx)
    }
    fn signed_scope(&self, principal: &Principal) -> Result<(String, String)> {
        if !principal.can_access() {
            return Err(IdentityError::InvalidInput);
        }
        let raw = json!({"v":1,"aud":self.audience,"exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"session","authProvider":"better-auth","subject":principal.user_id,"sessionID":principal.session_id}}).to_string();
        // The retained policy recognizes this legacy authority label; no external runtime is used.
        let signature = hex::encode(mac(self.secret.as_bytes(), raw.as_bytes()));
        Ok((raw, signature))
    }
}
