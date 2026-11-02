//! Placement: which cluster a project's workloads run on. Customers choose among the clusters the
//! operator offers (or one dedicated to them) while nothing runs in the project yet.
use super::*;

type Authority<'a> = Option<&'a webdock_browser_session::StoredAuthority>;
/// The database's refusals of a placement change, in the customer's words.
fn refusal(error: sea_orm::DbErr) -> PluginError {
    let database = match &error {
        sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(e))
        | sea_orm::DbErr::Exec(sea_orm::RuntimeErr::SqlxError(e)) => e.as_database_error(),
        _ => None,
    };
    match database.map(|e| {
        (
            e.code().unwrap_or_default().into_owned(),
            e.message().to_owned(),
        )
    }) {
        Some((code, message)) if code == "WD404" => {
            PluginError::new(webdock_plugin_protocol::ErrorCode::NotFound, message)
        }
        Some((code, message)) if code == "WD409" => conflict(&message),
        Some((code, _)) if code == "42501" => PluginError::denied(),
        _ => unavailable(),
    }
}
impl HostService {
    async fn session_tx(&self, actor: &Principal) -> Result<DatabaseTransaction> {
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| PluginError::denied())?;
        timeouts(&tx).await?;
        Ok(tx)
    }
    pub async fn placement(&self, actor: &Principal, project: &str) -> Result<Value> {
        let tx = self.session_tx(actor).await?;
        let value: Value = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.project_placement($1) AS value",
                [project.into()],
            ))
            .await
            .map_err(refusal)?
            .ok_or_else(PluginError::denied)?
            .try_get("", "value")
            .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(value)
    }
    pub async fn place(
        &self,
        actor: &Principal,
        project: &str,
        cluster: Option<&str>,
    ) -> Result<Value> {
        let tx = self.session_tx(actor).await?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.set_project_cluster($1,$2)",
            [project.into(), cluster.into()],
        ))
        .await
        .map_err(refusal)?;
        tx.commit().await.map_err(db_error)?;
        self.placement(actor, project).await
    }
    pub async fn clusters(&self, actor: &Principal, authority: Authority<'_>) -> Result<Value> {
        let guard = self.admin_guard(actor, authority, false).await?;
        let tx = self.operator_tx(actor).await?;
        let value: Value = tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_rls.operator_clusters() AS value",
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(unavailable)?
            .try_get("", "value")
            .map_err(db_error)?;
        self.admin_commit(tx, actor, authority, guard, false)
            .await?;
        Ok(value)
    }
    pub async fn offer_cluster(
        &self,
        actor: &Principal,
        authority: Authority<'_>,
        cluster: &str,
        offered: bool,
    ) -> Result<Value> {
        let guard = self.admin_guard(actor, authority, true).await?;
        let tx = self.operator_tx(actor).await?;
        let found: bool = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.offer_cluster($1,$2) AS found",
                [cluster.into(), offered.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(unavailable)?
            .try_get("", "found")
            .map_err(db_error)?;
        if !found {
            return Err(PluginError::new(
                webdock_plugin_protocol::ErrorCode::NotFound,
                "Cluster not found",
            ));
        }
        self.audit_admin(
            &tx,
            actor,
            if offered {
                "cluster.offer"
            } else {
                "cluster.withdraw"
            },
            cluster,
        )
        .await?;
        self.admin_commit(tx, actor, authority, guard, true).await?;
        Ok(json!({}))
    }
}
