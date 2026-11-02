//! What Core keeps from the agent's reports (see `webdock_rls.report_plugin_workload`): notices,
//! mailed once to the workspace's owners and admins and listed in the console, and log history.
use super::*;
use webdock_identity::{Delivery, DeliveryKind, DeliveryRequest};

impl HostService {
    /// Retention also runs when units are quiet or gone, and when mail is not configured.
    pub async fn prune_history(&self) -> Result<()> {
        let tx = self.controller().await?;
        tx.execute_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT webdock_rls.prune_plugin_history()",
        ))
        .await
        .map_err(db_error)?;
        tx.commit().await.map_err(db_error)
    }
    /// Mails notices that have lasted long enough; each recipient gets a fixed text and a link to
    /// sign in. Returns how many notices were handled.
    pub async fn dispatch_notifications(
        &self,
        delivery: &dyn Delivery,
        sign_in: &str,
    ) -> Result<usize> {
        let tx = self.controller().await?;
        let due: Value = tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_rls.due_notifications(10) AS due",
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(unavailable)?
            .try_get("", "due")
            .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        let notices = due.as_array().cloned().unwrap_or_default();
        for notice in &notices {
            let id = notice["id"].as_str().ok_or_else(unavailable)?;
            for email in notice["emails"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                // ponytail: a fresh id per mail, so a retry after a partial failure may mail some
                // recipients twice; key it per notice and recipient if that ever matters.
                let request = DeliveryRequest {
                    id: self
                        .ids
                        .allocate()
                        .await
                        .map_err(|_| unavailable())?
                        .to_string(),
                    email: email.to_owned(),
                    kind: DeliveryKind::Notice,
                    url: sign_in.to_owned(),
                };
                // A failed mail leaves the notice due; the next round tries again.
                delivery
                    .deliver(&request)
                    .await
                    .map_err(|_| unavailable())?;
            }
            let tx = self.controller().await?;
            tx.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.notification_mailed($1::bigint)",
                [id.into()],
            ))
            .await
            .map_err(db_error)?;
            tx.commit().await.map_err(db_error)?;
        }
        Ok(notices.len())
    }
    /// A workspace's open notices and those resolved in the last week, for its members.
    pub async fn notifications(&self, actor: &Principal, customer: &str) -> Result<Value> {
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| PluginError::denied())?;
        timeouts(&tx).await?;
        let value: Value = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.customer_notifications($1) AS value",
                [customer.into()],
            ))
            .await
            .map_err(|_| PluginError::denied())?
            .ok_or_else(PluginError::denied)?
            .try_get("", "value")
            .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(value)
    }
    /// A resource's stored log lines, newest first, for owners, admins and operators.
    pub async fn logs(
        &self,
        actor: &Principal,
        project: &str,
        plugin: &str,
        resource: &str,
        search: &str,
        before: Option<&str>,
    ) -> Result<Value> {
        if search.chars().count() > 200
            || resource.len() > 128
            || !slug(plugin)
            || before.is_some_and(|b| !b.parse::<i64>().is_ok_and(|n| n > 0))
        {
            return Err(invalid("Invalid log query"));
        }
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| PluginError::denied())?;
        timeouts(&tx).await?;
        let value: Value = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.resource_logs($1,$2,$3,$4,$5::bigint) AS value",
                [
                    project.into(),
                    plugin.into(),
                    resource.into(),
                    search.into(),
                    before.into(),
                ],
            ))
            .await
            .map_err(|_| PluginError::denied())?
            .ok_or_else(PluginError::denied)?
            .try_get("", "value")
            .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(value)
    }
}
