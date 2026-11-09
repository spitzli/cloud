//! Domain-specific encrypted browser sidecar storage; OAuth authority stays with OAuthService.
use super::BrowserSessions;
use crate::{Error, Flow, Result};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio_postgres::types::Type;
use webdock_persistence::postgres::Connection;
const BUDGET: Duration = Duration::from_secs(30);
pub(super) struct NewBrowserSession {
    pub reference: String,
    pub origin: String,
    pub user: String,
    pub session: String,
    pub payload: String,
    pub expires: i64,
}
pub(super) struct RemovedBrowserSession {
    pub user: String,
    pub session: String,
    pub payload: String,
}
impl BrowserSessions {
    async fn store_connection(&self) -> Result<Connection> {
        self.native_store
            .as_ref()
            .ok_or(Error::Configuration)?
            .acquire()
            .await
            .map_err(|_| Error::Configuration)
    }
    pub(super) async fn persist_flow(
        &self,
        reference: &str,
        origin: &str,
        payload: &str,
    ) -> Result<()> {
        self.config.app(origin)?;
        if self.native_store.is_none() {
            self.db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.identity_browser_flow(token_hash,app_origin,client_id,payload,expires_at) VALUES($1,$2,$3,$4,statement_timestamp()+interval '5 minutes')",[reference.into(),origin.into(),self.config.client_id.clone().into(),payload.into()])).await?;
            return Ok(());
        }
        tokio::time::timeout(BUDGET,async{let mut connection=self.store_connection().await?;let tx=connection.transaction().await.map_err(|_|Error::Configuration)?;
   if tx.execute_typed("INSERT INTO webdock_auth.identity_browser_flow(token_hash,app_origin,client_id,payload,expires_at) VALUES($1,$2,$3,$4,statement_timestamp()+interval '5 minutes')",&[(&reference,Type::TEXT),(&origin,Type::TEXT),(&self.config.client_id,Type::TEXT),(&payload,Type::TEXT)]).await.map_err(|_|Error::Configuration)?!=1{return Err(Error::Configuration);}
   tx.commit().await.map_err(|_|Error::Configuration)
  }).await.map_err(|_|Error::Configuration)?
    }
    pub(super) async fn consume_native_flow(
        &self,
        reference: &str,
        origin: &str,
        state: &str,
    ) -> Result<Flow> {
        self.config.app(origin)?;
        tokio::time::timeout(BUDGET,async{let mut connection=self.store_connection().await?;let tx=connection.transaction().await.map_err(|_|Error::Configuration)?;
   let row=tx.query_typed_opt("SELECT payload FROM webdock_auth.identity_browser_flow WHERE token_hash=$1 AND app_origin=$2 AND client_id=$3 AND expires_at>statement_timestamp() FOR UPDATE",&[(&reference,Type::TEXT),(&origin,Type::TEXT),(&self.config.client_id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.ok_or(Error::Unauthorized)?;
   let encrypted:String=row.try_get("payload").map_err(|_|Error::Configuration)?;let flow:Flow=serde_json::from_str(&self.keys.decrypt(&encrypted)?).map_err(|_|Error::Unauthorized)?;
   if !bool::from(flow.state.as_bytes().ct_eq(state.as_bytes()))||flow.origin!=origin{return Err(Error::Unauthorized);}
   if tx.execute_typed("DELETE FROM webdock_auth.identity_browser_flow WHERE token_hash=$1 AND app_origin=$2 AND client_id=$3 AND expires_at>statement_timestamp()",&[(&reference,Type::TEXT),(&origin,Type::TEXT),(&self.config.client_id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?!=1{return Err(Error::Unauthorized);}
   tx.commit().await.map_err(|_|Error::Configuration)?;Ok(flow)
  }).await.map_err(|_|Error::Configuration)?
    }
    pub(super) async fn persist_browser_session(
        &self,
        record: NewBrowserSession,
        ignore_existing: bool,
    ) -> Result<()> {
        self.config.app(&record.origin)?;
        let sql = if ignore_existing {
            "INSERT INTO webdock_auth.identity_browser_session(token_hash,app_origin,client_id,user_id,session_id,payload,expires_at) VALUES($1,$2,$3,$4,$5,$6,to_timestamp($7::double precision)) ON CONFLICT(token_hash) DO NOTHING"
        } else {
            "INSERT INTO webdock_auth.identity_browser_session(token_hash,app_origin,client_id,user_id,session_id,payload,expires_at) VALUES($1,$2,$3,$4,$5,$6,to_timestamp($7::double precision))"
        };
        if self.native_store.is_none() {
            self.db
                .execute_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    sql,
                    [
                        record.reference.into(),
                        record.origin.into(),
                        self.config.client_id.clone().into(),
                        record.user.into(),
                        record.session.into(),
                        record.payload.into(),
                        record.expires.into(),
                    ],
                ))
                .await?;
            return Ok(());
        }
        tokio::time::timeout(BUDGET, async {
            let mut connection = self.store_connection().await?;
            let tx = connection
                .transaction()
                .await
                .map_err(|_| Error::Configuration)?;
            let count = tx
                .execute_typed(
                    sql,
                    &[
                        (&record.reference, Type::TEXT),
                        (&record.origin, Type::TEXT),
                        (&self.config.client_id, Type::TEXT),
                        (&record.user, Type::TEXT),
                        (&record.session, Type::TEXT),
                        (&record.payload, Type::TEXT),
                        (&record.expires, Type::INT8),
                    ],
                )
                .await
                .map_err(|_| Error::Configuration)?;
            if count != 1 && !(ignore_existing && count == 0) {
                return Err(Error::Configuration);
            }
            tx.commit().await.map_err(|_| Error::Configuration)
        })
        .await
        .map_err(|_| Error::Configuration)?
    }
    pub(super) async fn remove_browser_session(
        &self,
        reference: &str,
        origin: &str,
    ) -> Result<Option<RemovedBrowserSession>> {
        self.config.app(origin)?;
        if self.native_store.is_none() {
            return self.db.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"DELETE FROM webdock_auth.identity_browser_session WHERE token_hash=$1 AND app_origin=$2 AND client_id=$3 RETURNING payload,user_id,session_id",[reference.into(),origin.into(),self.config.client_id.clone().into()])).await?.map(|row|Ok(RemovedBrowserSession{payload:row.try_get("","payload")?,user:row.try_get("","user_id")?,session:row.try_get("","session_id")?})).transpose();
        }
        tokio::time::timeout(BUDGET,async{let mut connection=self.store_connection().await?;let tx=connection.transaction().await.map_err(|_|Error::Configuration)?;
   let row=tx.query_typed_opt("DELETE FROM webdock_auth.identity_browser_session WHERE token_hash=$1 AND app_origin=$2 AND client_id=$3 RETURNING payload,user_id,session_id",&[(&reference,Type::TEXT),(&origin,Type::TEXT),(&self.config.client_id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?;
   let record=row.map(|row|->std::result::Result<_,tokio_postgres::Error>{Ok(RemovedBrowserSession{payload:row.try_get("payload")?,user:row.try_get("user_id")?,session:row.try_get("session_id")?})}).transpose().map_err(|_|Error::Configuration)?;
   tx.commit().await.map_err(|_|Error::Configuration)?;Ok(record)
  }).await.map_err(|_|Error::Configuration)?
    }
    pub(super) async fn logout_flow_payload(&self, reference: &str) -> Result<String> {
        if self.native_store.is_none() {
            return Ok(self.db.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT payload FROM webdock_auth.identity_browser_flow WHERE token_hash=$1 AND client_id=$2 AND expires_at>statement_timestamp()",[reference.into(),self.config.client_id.clone().into()])).await?.ok_or(Error::Unauthorized)?.try_get("","payload")?);
        }
        tokio::time::timeout(BUDGET,async{let mut connection=self.store_connection().await?;let tx=connection.transaction().await.map_err(|_|Error::Configuration)?;
   let payload=tx.query_typed_opt("SELECT payload FROM webdock_auth.identity_browser_flow WHERE token_hash=$1 AND client_id=$2 AND expires_at>statement_timestamp()",&[(&reference,Type::TEXT),(&self.config.client_id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?.ok_or(Error::Unauthorized)?.try_get("payload").map_err(|_|Error::Configuration)?;
   tx.commit().await.map_err(|_|Error::Configuration)?;Ok(payload)
  }).await.map_err(|_|Error::Configuration)?
    }
    pub(super) async fn consume_logout_flow(&self, reference: &str) -> Result<bool> {
        if self.native_store.is_none() {
            let tx = self.db.begin().await?;
            let locked=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT token_hash FROM webdock_auth.identity_browser_flow WHERE token_hash=$1 AND client_id=$2 AND expires_at>statement_timestamp() FOR UPDATE",[reference.into(),self.config.client_id.clone().into()])).await?;
            if locked.is_none() {
                tx.rollback().await?;
                return Ok(false);
            }
            let changed=tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"DELETE FROM webdock_auth.identity_browser_flow WHERE token_hash=$1 AND client_id=$2 AND expires_at>statement_timestamp()",[reference.into(),self.config.client_id.clone().into()])).await?.rows_affected()==1;
            tx.commit().await?;
            return Ok(changed);
        }
        tokio::time::timeout(BUDGET,async{let mut connection=self.store_connection().await?;let tx=connection.transaction().await.map_err(|_|Error::Configuration)?;
   let locked=tx.query_typed_opt("SELECT token_hash FROM webdock_auth.identity_browser_flow WHERE token_hash=$1 AND client_id=$2 AND expires_at>statement_timestamp() FOR UPDATE",&[(&reference,Type::TEXT),(&self.config.client_id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?;
   if locked.is_none(){tx.rollback().await.map_err(|_|Error::Configuration)?;return Ok(false);}
   let changed=tx.execute_typed("DELETE FROM webdock_auth.identity_browser_flow WHERE token_hash=$1 AND client_id=$2 AND expires_at>statement_timestamp()",&[(&reference,Type::TEXT),(&self.config.client_id,Type::TEXT)]).await.map_err(|_|Error::Configuration)?==1;
   tx.commit().await.map_err(|_|Error::Configuration)?;Ok(changed)
  }).await.map_err(|_|Error::Configuration)?
    }
    pub(super) async fn prune_native_store(&self, limit: u32) -> Result<u64> {
        tokio::time::timeout(BUDGET,async{let mut connection=self.store_connection().await?;let tx=connection.transaction().await.map_err(|_|Error::Configuration)?;let limit=i64::from(limit);let mut total=0;
   for table in ["identity_browser_flow","identity_browser_session"]{let sql=format!("WITH expired AS (SELECT token_hash FROM webdock_auth.{table} WHERE expires_at<=statement_timestamp() ORDER BY expires_at LIMIT $1 FOR UPDATE SKIP LOCKED) DELETE FROM webdock_auth.{table} t USING expired e WHERE t.token_hash=e.token_hash");total+=tx.execute_typed(&sql,&[(&limit,Type::INT8)]).await.map_err(|_|Error::Configuration)?;}
   tx.commit().await.map_err(|_|Error::Configuration)?;Ok(total)
  }).await.map_err(|_|Error::Configuration)?
    }
}
