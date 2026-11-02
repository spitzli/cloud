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
        if !principal.can_access() {
            return Err(IdentityError::InvalidInput);
        }
        let raw = json!({"v":1,"aud":self.audience,"exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"session","authProvider":"better-auth","subject":principal.user_id,"sessionID":principal.session_id}}).to_string();
        // The retained policy recognizes this legacy authority label; no external runtime is used.
        let signature = hex::encode(mac(self.secret.as_bytes(), raw.as_bytes()));
        tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)", [raw.into(), signature.into()])).await?;
        Ok(())
    }
}
