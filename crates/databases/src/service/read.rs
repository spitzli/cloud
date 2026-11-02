//! Authenticated bearer reads; no worker, provisioning, secret material or persisted authority.
use super::*;
use webdock_oauth::{McpAuthentication, OAuthService};

impl DatabaseService {
    /// OAuth's server-owned reference is revalidated and retained through the business commit.
    /// The principal supplied alongside the reference is never used as authentication evidence.
    pub async fn execute_mcp_read(
        &self,
        oauth: &Arc<OAuthService>,
        actor: &McpAuthentication,
        input: Value,
    ) -> Result<Value> {
        let command = DatabaseReadCommand::parse(input)?;
        if actor.authority.registry_scope() {
            return Err(read_denied());
        }
        let (tx, guard) = self
            .retain_mcp(oauth, &actor.authority)
            .await
            .map_err(read_guard_error)?;
        let current = guard.principal();
        self.installed(&tx, false).await?;
        let value = self.read_command(&tx, current, &command).await?;
        if serde_json::to_vec(&value).map_err(|_| unavailable())?.len() > 262144 {
            return Err(conflict(
                "Metadata page is too large. Request a smaller limit.",
            ));
        }
        guard.finish(self, tx).await.map_err(read_guard_error)?;
        // finish retains the real guard through commit; no cookie or LocalSession bridge.
        Ok(value)
    }
    async fn authorize_read(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        project: Option<&str>,
        customer: Option<&str>,
    ) -> Result<Value> {
        let customer = if let Some(project) = project {
            let row=one(tx,"SELECT jsonb_build_object('customerID',customer_id) AS value FROM webdock_admin.projects WHERE id=$1",vec![project.into()]).await?;
            Some(text(&row, "customerID")?.to_owned())
        } else {
            customer.map(str::to_owned)
        };
        if let Some(customer) = customer.as_deref() {
            // Access membership mutations lock organization first. Retain that same fence,
            // exact mapping and current member before the ordinary project/customer checks.
            let org=one(tx,"SELECT jsonb_build_object('id',o.id) AS value FROM webdock_auth.organization o JOIN webdock_auth.tenant_customer t ON t.organization_id=o.id WHERE t.customer_id=$1 FOR SHARE OF o,t",vec![customer.into()]).await?;
            if !actor.is_operator() {
                one(tx,"SELECT jsonb_build_object('id',m.id) AS value FROM webdock_auth.member m WHERE m.\"organizationId\"=$1 AND m.\"userId\"=$2 FOR SHARE OF m",vec![text(&org,"id")?.into(),actor.user_id.clone().into()]).await?;
            }
        }
        let access = self
            .authorize_ceiling(
                tx,
                actor,
                project,
                customer.as_deref(),
                false,
                project.is_none() && customer.is_none(),
                true,
                false,
            )
            .await?;
        if customer
            .as_deref()
            .is_some_and(|id| access["customerID"] != id)
        {
            return Err(denied());
        }
        Ok(access)
    }
    async fn read_access(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        id: &str,
    ) -> Result<Value> {
        let row = one(tx, "SELECT jsonb_build_object('projectID',project_id,'kind',kind) AS value FROM webdock_auth.hosting_database WHERE id=$1", vec![id.into()]).await?;
        self.authorize_read(tx, actor, Some(text(&row, "projectID")?), None)
            .await?;
        Ok(row)
    }
    async fn read_command(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        read: &DatabaseReadCommand,
    ) -> Result<Value> {
        let cmd = read.command.value();
        let cursor = &read.cursor;
        let fetch = (read.limit + 1) as i64;
        match read.action() {
            "databases.projects.list" => {
                let customer = text(cmd, "customerID")?;
                self.authorize_read(tx, actor, None, Some(customer)).await?;
                let rows=all(tx,"SELECT jsonb_build_object('id',p.id,'name',p.name,'count',(SELECT count(*)::int FROM webdock_auth.hosting_database d WHERE d.project_id=p.id AND d.state<>'purged')) AS value FROM webdock_admin.projects p WHERE p.customer_id=$1 AND p.status='active' AND p.id::bigint>$2::bigint ORDER BY p.id::bigint LIMIT $3",vec![customer.into(),cursor.clone().into(),fetch.into()]).await?;
                page(rows, read.limit)
            }
            "databases.list" => {
                let project = text(cmd, "projectID")?;
                self.authorize_read(tx, actor, Some(project), None).await?;
                let rows=all(tx,"SELECT jsonb_build_object('id',id) AS value FROM webdock_auth.hosting_database WHERE project_id=$1 AND id::bigint>$2::bigint ORDER BY id::bigint LIMIT $3",vec![project.into(),cursor.clone().into(),fetch.into()]).await?;
                let mut views = Vec::with_capacity(rows.len());
                for row in rows {
                    views.push(self.managed_view(tx, text(&row, "id")?).await?);
                }
                page(views, read.limit)
            }
            "databases.get" => {
                let id = text(cmd, "databaseID")?;
                self.read_access(tx, actor, id).await?;
                self.managed_view(tx, id).await
            }
            "databases.permissions" => {
                let access = self
                    .authorize_read(tx, actor, Some(text(cmd, "projectID")?), None)
                    .await?;
                Ok(json!({"operator":access["operator"],"canWrite":false}))
            }
            "databases.plans.list" => {
                let project = cmd.get("projectID").and_then(Value::as_str);
                let access = self.authorize_read(tx, actor, project, None).await?;
                let rows=all(tx,"SELECT jsonb_build_object('id',id,'plan',plan,'availability',availability,'revision',revision) AS value FROM webdock_auth.hosting_database_plan WHERE ($1 OR (availability='public' AND NOT(plan ? 'sponsoredCustomerID'))) AND id::bigint>$2::bigint ORDER BY id::bigint LIMIT $3",vec![(access["operator"]==true).into(),cursor.clone().into(),fetch.into()]).await?;
                page(rows, read.limit)
            }
            "databases.backupPolicies.get" => {
                let id = text(cmd, "databaseID")?;
                let db = self.read_access(tx, actor, id).await?;
                if db["kind"] != "dedicated" {
                    return Err(conflict(
                        "Shared PostgreSQL backup and recovery are unavailable.",
                    ));
                }
                // Explicit metadata projection: the policy's provider destination and authority stay private.
                let policy=optional(tx,"SELECT jsonb_build_object('id',id,'revision',revision,'revokedAt',revoked_at,'observedRevision',observed_revision,'observedSuspended',observed_suspended,'observedAt',observed_at,'observedControls',observed_controls,'revocationEffective',revoked_at IS NOT NULL AND observed_suspended AND observed_revision=revision AND observed_at>clock_timestamp()-interval '90 seconds') AS value FROM webdock_auth.hosting_database_backup_policy WHERE database_id=$1",vec![id.into()]).await?;
                let rows=all(tx,"SELECT jsonb_build_object('id',b.id,'policyID',b.policy_id,'baseBackupID',b.base_backup_id,'completedAt',b.completed_at) AS value FROM webdock_auth.hosting_database_backup b JOIN webdock_auth.hosting_database_backup_policy p ON p.id=b.policy_id WHERE p.database_id=$1 AND b.id::bigint>$2::bigint ORDER BY b.id::bigint LIMIT $3",vec![id.into(),cursor.clone().into(),fetch.into()]).await?;
                Ok(
                    json!({"policy":policy,"backup":self.backup_view(tx,id).await?,"backups":page(rows,read.limit)?}),
                )
            }
            "databases.operations.get" => {
                let id = text(cmd, "databaseID")?;
                self.read_access(tx, actor, id).await?;
                self.read_operation(tx, id, text(cmd, "operationID")?).await
            }
            _ => Err(invalid()),
        }
    }
    pub(super) async fn read_operation(
        &self,
        tx: &DatabaseTransaction,
        database: &str,
        operation: &str,
    ) -> Result<Value> {
        one(tx,"SELECT jsonb_build_object('id',o.id,'databaseID',d.id,'projectID',o.project_id,'action',o.action,'status',o.status,'targetRevision',o.target_revision,'createdAt',o.created_at) AS value FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_database d ON d.id=$1 AND d.customer_id=o.customer_id AND d.project_id=o.project_id WHERE o.id=$2 AND (d.operation_id=o.id OR EXISTS(SELECT 1 FROM webdock_auth.hosting_database_job j WHERE j.operation_id=o.id AND j.database_id=d.id))",vec![database.into(),operation.into()]).await
    }
}
fn read_denied() -> Error {
    Error {
        status: 403,
        message: "Current hosting read authority without preview is required.",
    }
}
fn page(mut rows: Vec<Value>, limit: u64) -> Result<Value> {
    let more = rows.len() > limit as usize;
    if more {
        rows.truncate(limit as usize);
    }
    let next = if more {
        Some(text(rows.last().ok_or_else(unavailable)?, "id")?.to_owned())
    } else {
        None
    };
    Ok(json!({"items":rows,"nextCursor":next}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn page_has_explicit_continuation_and_preserves_last_item() {
        assert_eq!(
            page(
                vec![json!({"id":"1"}), json!({"id":"2"}), json!({"id":"3"})],
                2
            )
            .unwrap(),
            json!({"items":[{"id":"1"},{"id":"2"}],"nextCursor":"2"})
        );
        assert_eq!(
            page(vec![json!({"id":"3"})], 2).unwrap()["nextCursor"],
            Value::Null
        );
        assert_eq!(
            page(vec![], 2).unwrap(),
            json!({"items":[],"nextCursor":null})
        );
    }
}

fn read_guard_error(error: Error) -> Error {
    if error.status >= 500 {
        error
    } else {
        read_denied()
    }
}
#[cfg(test)]
mod retained_authentication_tests {
    use super::*;
    #[test]
    fn read_database_failure_is_transient_not_original_authority_revocation() {
        let error = read_guard_error(super::super::authority::oauth_failure(
            webdock_oauth::Error::Database(sea_orm::DbErr::Custom(
                "isolated transient test only".into(),
            )),
        ));
        assert_eq!(error.status, 503);
        assert_eq!(
            read_guard_error(super::super::authority::oauth_failure(
                webdock_oauth::Error::AccessDenied
            ))
            .status,
            403
        );
    }
}
