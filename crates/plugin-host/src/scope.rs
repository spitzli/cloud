use crate::{db_error, invalid};
use hmac::{Hmac, Mac};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement};
use serde_json::{Value, json};
use sha2::Sha256;
use webdock_plugin_protocol::Result;
/// This signer has no public raw-scope entry point.
pub(crate) struct HostScopes {
    secret: String,
}
impl HostScopes {
    pub(crate) fn new(secret: String) -> Result<Self> {
        if secret.len() < 32 {
            return Err(invalid("Invalid RLS signing configuration"));
        }
        Ok(Self { secret })
    }
    async fn apply(&self, tx: &DatabaseTransaction, audience: &str, scope: Value) -> Result<()> {
        let raw =
            json!({"v":1,"aud":audience,"exp":chrono::Utc::now().timestamp()+60,"scope":scope})
                .to_string();
        let mut mac = Hmac::<Sha256>::new_from_slice(self.secret.as_bytes())
            .map_err(|_| invalid("Invalid RLS configuration"))?;
        mac.update(raw.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());
        tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",[raw.into(),signature.into()])).await.map_err(db_error)?;
        Ok(())
    }
    pub(crate) async fn controller(&self, tx: &DatabaseTransaction) -> Result<()> {
        self.apply(
            tx,
            "webdock_worker_runtime",
            json!({"kind":"system","purpose":"plugin-controller","authProvider":"better-auth"}),
        )
        .await
    }
    pub(crate) async fn invocation(
        &self,
        tx: &DatabaseTransaction,
        invocation: &super::authority::PersistedInvocation,
    ) -> Result<()> {
        self.apply(tx,"webdock_auth_runtime",json!({"kind":"session","authProvider":"better-auth","subject":invocation.actor,"sessionID":invocation.session,"pluginInvocationID":invocation.id})).await
    }
}
