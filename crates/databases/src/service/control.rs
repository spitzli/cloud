use super::*;
impl DatabaseService {
    /// Core disables retired offerings at startup; library callers retain their existing default.
    /// This affects new allocations only, never accepted-order replay or worker execution.
    pub fn with_new_orders_enabled(mut self, enabled: bool) -> Self {
        self.new_orders_enabled = enabled;
        self
    }

    pub(super) fn require_new_orders(&self) -> Result<()> {
        if !self.new_orders_enabled {
            return Err(conflict(
                "New shared and self-hosted PostgreSQL orders are retired. Use your own Neon account for a new database; existing databases remain available.",
            ));
        }
        Ok(())
    }

    pub fn with_management(mut self, config: ManagementConfig) -> Result<Self> {
        for id in [&config.cluster_id, &config.protected_database_id]
            .into_iter()
            .flatten()
        {
            if id.starts_with('0') || id.parse::<i64>().is_err() {
                return Err(invalid());
            }
        }
        if config
            .lease_secret
            .as_ref()
            .is_some_and(|key| key.len() < 32 || key.len() > 4096)
        {
            return Err(invalid());
        }
        self.management = Some(config);
        Ok(self)
    }
}

#[derive(Default)]
pub(super) struct ReceiptScope<'a> {
    pub customer: Option<&'a str>,
    pub project: Option<&'a str>,
    pub cluster: Option<&'a str>,
}
impl DatabaseService {
    pub async fn execute_managed(
        &self,
        principal: &Principal,
        authority: &StoredAuthority,
        input: Value,
    ) -> Result<Value> {
        let command = ManagedCommand::parse(input)?;
        let read = command.is_read();
        let cmd = command.into_value();
        // Retain the existing ordinary-order worker before the session fence/auth.
        // Atomic restore's additional early epoch admission remains unactivated.
        let order_worker = if cmd["action"] == "databases.acceptOrder" {
            Some(self.worker.begin().await?)
        } else {
            None
        };
        let (tx, guard) = self.retain_browser(principal, authority).await?;
        let current = guard.principal();
        if !read && guard.read_only() {
            return Err(denied());
        }
        self.installed(&tx, !read).await?;
        let result = if cmd["action"] == "databases.acceptOrder" {
            let project = text(&cmd, "projectID")?;
            let access = self
                .authorize_mode(
                    &tx,
                    current,
                    Some(project),
                    None,
                    true,
                    false,
                    authority,
                    true,
                )
                .await?;
            let offer=one(&tx,"SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_database_offer o WHERE id=$1 AND project_id=$2 AND customer_id=$3",vec![text(&cmd,"offerID")?.into(),project.into(),text(&access,"customerID")?.into()]).await?;
            let cluster = if let Some(source) = offer["snapshot"]
                .get("sourceDatabaseID")
                .and_then(Value::as_str)
            {
                let row=one(&tx,"SELECT jsonb_build_object('cluster',b.cluster_id) AS value FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id WHERE d.id=$1 AND d.project_id=$2 AND d.customer_id=$3",vec![source.into(),project.into(),text(&access,"customerID")?.into()]).await?;
                text(&row, "cluster")?.to_owned()
            } else {
                self.management
                    .as_ref()
                    .and_then(|m| m.cluster_id.clone())
                    .ok_or_else(unavailable)?
            };
            let worker = order_worker.ok_or_else(unavailable)?;
            self.apply_worker_scope(&worker,json!({"kind":"system","purpose":"database-order","clusterID":cluster,"customerID":access["customerID"],"subject":current.user_id,"sessionID":current.session_id})).await?;
            let result = self
                .accept_order(&worker, current, authority, &cmd, &access, &cluster)
                .await?;
            self.authorize_mode(
                &tx,
                current,
                Some(project),
                None,
                true,
                false,
                authority,
                true,
            )
            .await?;
            guard.verify(self).await?;
            worker.commit().await?;
            result
        } else {
            self.control_command(&tx, current, authority, &cmd).await?
        };
        guard.finish(self, tx).await?;
        Ok(result)
    }
    async fn control_command(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        authority: &StoredAuthority,
        cmd: &Value,
    ) -> Result<Value> {
        let action = text(cmd, "action")?;
        match action {
            "databases.backupPolicies.grant"
            | "databases.backupPolicies.update"
            | "databases.backupPolicies.get"
            | "databases.backupPolicies.revoke"
            | "databases.backupDestinations.register"
            | "databases.backup" => self.backup_command(tx, actor, authority, cmd).await,
            "databases.projects.list" => {
                let customer = text(cmd, "customerID")?;
                self.authorize_mode(
                    tx,
                    actor,
                    None,
                    Some(customer),
                    false,
                    false,
                    authority,
                    false,
                )
                .await?;
                Ok(Value::Array(all(tx,"SELECT jsonb_build_object('id',p.id,'name',p.name,'count',(SELECT count(*)::int FROM webdock_auth.hosting_database d WHERE d.customer_id=$1 AND d.project_id=p.id AND d.state<>'purged')) AS value FROM webdock_admin.projects p WHERE p.customer_id=$1 AND p.status='active' ORDER BY p.name,p.id LIMIT 200",vec![customer.into()]).await?))
            }
            "databases.permissions" => {
                let a = self
                    .authorize_mode(
                        tx,
                        actor,
                        Some(text(cmd, "projectID")?),
                        None,
                        false,
                        false,
                        authority,
                        false,
                    )
                    .await?;
                Ok(
                    json!({"canWrite":a["canWrite"]==true&&!authority.read_only(),"operator":a["operator"]}),
                )
            }
            "databases.list" => {
                let project = text(cmd, "projectID")?;
                self.authorize_mode(
                    tx,
                    actor,
                    Some(project),
                    None,
                    false,
                    false,
                    authority,
                    false,
                )
                .await?;
                let ids=all(tx,"SELECT jsonb_build_object('id',id) AS value FROM webdock_auth.hosting_database WHERE project_id=$1 ORDER BY created_at,id",vec![project.into()]).await?;
                let mut result = vec![];
                for id in ids {
                    result.push(self.managed_view(tx, text(&id, "id")?).await?)
                }
                Ok(Value::Array(result))
            }
            "databases.operations.get" => {
                let id = text(cmd, "databaseID")?;
                self.control_access(tx, actor, authority, id, false, false, false)
                    .await?;
                self.read_operation(tx, id, text(cmd, "operationID")?).await
            }
            "databases.get" => {
                let id = text(cmd, "databaseID")?;
                self.control_access(tx, actor, authority, id, false, false, false)
                    .await?;
                self.managed_view(tx, id).await
            }
            "databases.plans.list" => {
                let project = cmd.get("projectID").and_then(Value::as_str);
                let a = self
                    .authorize_mode(
                        tx,
                        actor,
                        project,
                        None,
                        false,
                        project.is_none(),
                        authority,
                        false,
                    )
                    .await?;
                Ok(Value::Array(all(tx,"SELECT jsonb_build_object('id',id,'plan',plan,'availability',availability,'revision',revision) AS value FROM webdock_auth.hosting_database_plan WHERE $1 OR (availability='public' AND NOT(plan ? 'sponsoredCustomerID')) ORDER BY created_at,id",vec![(a["operator"]==true).into()]).await?))
            }
            "databases.plans.create" | "databases.plans.setAvailability" => {
                self.authorize_mode(tx, actor, None, None, true, true, authority, true)
                    .await?;
                let (hash, prior) = self.replay_control(tx, actor, cmd).await?;
                if let Some(value) = prior {
                    return Ok(value);
                }
                let value = if action == "databases.plans.create" {
                    one(tx,"INSERT INTO webdock_auth.hosting_database_plan(id,plan) VALUES($1,$2) RETURNING jsonb_build_object('id',id,'plan',plan,'availability',availability,'revision',revision) AS value",vec![self.id().await?.into(),cmd["plan"].clone().into()]).await?
                } else {
                    if cmd["availability"] == "public"
                        && !self.management.as_ref().is_some_and(|c| c.public_ordering)
                    {
                        return Err(conflict(
                            "Managed PostgreSQL is not approved for public ordering.",
                        ));
                    }
                    optional(tx,"UPDATE webdock_auth.hosting_database_plan SET availability=$2,revision=revision+1 WHERE id=$1 AND revision=$3 AND NOT($2='public' AND plan ? 'sponsoredCustomerID') RETURNING jsonb_build_object('id',id,'plan',plan,'availability',availability,'revision',revision) AS value",vec![text(cmd,"planRevisionID")?.into(),text(cmd,"availability")?.into(),number(cmd,"revision")?.into()]).await?.ok_or_else(||conflict("Database plan changed. Refresh and try again."))?
                };
                self.record_control(tx, actor, cmd, &hash, &value, ReceiptScope::default())
                    .await?;
                self.control_audit(tx, actor, action, text(&value, "id")?)
                    .await?;
                Ok(value)
            }
            "databases.offers.create" => {
                let project = text(cmd, "projectID")?;
                let access = self
                    .authorize_mode(tx, actor, Some(project), None, true, false, authority, true)
                    .await?;
                let customer = text(&access, "customerID")?;
                let (hash, prior) = self.replay_control(tx, actor, cmd).await?;
                if let Some(value) = prior {
                    return Ok(value);
                }
                if cmd.get("databaseID").is_none() {
                    self.require_new_orders()?;
                }
                let plan=optional(tx,"SELECT to_jsonb(p) AS value FROM webdock_auth.hosting_database_plan p WHERE id=$1 FOR SHARE",vec![text(cmd,"planRevisionID")?.into()]).await?.ok_or_else(||conflict("This database plan is not available."))?;
                self.plan_available(&plan, customer, access["operator"] == true)?;
                let mut snapshot = plan["plan"].clone();
                snapshot["backupStatus"] = json!("not-configured");
                snapshot["billing"] = json!(if snapshot.get("sponsoredCustomerID").is_some() {
                    "sponsored"
                } else {
                    "monthly-net-fixed"
                });
                snapshot["billingStarts"] = json!("observed-ready");
                if let Some(source) = cmd.get("databaseID").and_then(Value::as_str) {
                    let d = self
                        .control_access(tx, actor, authority, source, true, false, true)
                        .await?;
                    self.protect_database(source, "upgrade")?;
                    if d["project_id"] != project
                        || d["state"] != "ready"
                        || !d["pending_order_id"].is_null()
                    {
                        return Err(conflict(
                            "Only a ready database can accept an upgrade offer.",
                        ));
                    }
                    snapshot["sourceDatabaseID"] = json!(source);
                    snapshot["sourceRevision"] = d["revision"].clone();
                    snapshot["requiresInterruption"] = json!(true);
                }
                let mut value=one(tx,"INSERT INTO webdock_auth.hosting_database_offer(id,customer_id,project_id,plan_revision_id,name,snapshot) VALUES($1,$2,$3,$4,$5,$6) RETURNING jsonb_build_object('id',id,'name',name,'snapshot',snapshot,'expiresAt',expires_at) AS value",vec![self.id().await?.into(),customer.into(),project.into(),text(cmd,"planRevisionID")?.into(),text(cmd,"name")?.into(),snapshot.into()]).await?;
                value["expiresAt"] = iso(&value["expiresAt"])?;
                self.record_control(
                    tx,
                    actor,
                    cmd,
                    &hash,
                    &value,
                    ReceiptScope {
                        customer: Some(customer),
                        project: Some(project),
                        cluster: None,
                    },
                )
                .await?;
                Ok(value)
            }
            "databases.pools.register" => self.register_pool(tx, actor, authority, cmd).await,
            "databases.installations.list" => {
                self.authorize_mode(tx, actor, None, None, false, true, authority, false)
                    .await?;
                Ok(Value::Array(all(tx,"SELECT jsonb_build_object('id',id,'clusterID',cluster_id,'kind',kind,'node',node,'poolID',pool_id,'revision',revision,'observedRevision',observed_revision,'state',state,'cnpgUID',cnpg_uid,'demand',demand,'maxDatabases',max_databases,'logicalCapacityBytes',logical_capacity_bytes::text,'host',host,'directPort',direct_port,'pooledPort',pooled_port,'observedAt',observed_at) AS value FROM webdock_auth.hosting_database_backend WHERE cluster_id=$1 ORDER BY id",vec![text(cmd,"clusterID")?.into()]).await?))
            }
            "databases.installations.create" => {
                self.authorize_mode(tx, actor, None, None, true, true, authority, true)
                    .await?;
                let (hash, prior) = self.replay_control(tx, actor, cmd).await?;
                if let Some(value) = prior {
                    return Ok(value);
                }
                let cluster = text(cmd, "clusterID")?;
                let backend = self
                    .create_backend(
                        tx,
                        cluster,
                        "shared-pool",
                        None,
                        Some((text(cmd, "node")?, text(cmd, "poolID")?)),
                    )
                    .await?;
                let operation = self
                    .enqueue_database(
                        tx,
                        actor,
                        authority,
                        text(&backend, "id")?,
                        None,
                        "create",
                        1,
                        true,
                    )
                    .await?;
                let result = json!({"installationID":backend["id"],"operationID":operation,"state":"provisioning"});
                self.record_control(
                    tx,
                    actor,
                    cmd,
                    &hash,
                    &result,
                    ReceiptScope {
                        cluster: Some(cluster),
                        ..Default::default()
                    },
                )
                .await?;
                self.control_audit(tx, actor, action, text(&backend, "id")?)
                    .await?;
                Ok(result)
            }
            "databases.restore" => self.restore_backup(tx, actor, authority, cmd).await,
            "databases.clone" => {
                let id = cmd
                    .get("databaseID")
                    .or_else(|| cmd.get("sourceDatabaseID"))
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?;
                self.control_access(tx, actor, authority, id, true, false, true)
                    .await?;
                Err(conflict("Backups and recovery are not configured."))
            }
            "databases.migrations.list"
            | "databases.migrationPurgePreview"
            | "databases.purgeMigrationSource" => {
                self.migration_maintenance(tx, actor, authority, cmd).await
            }
            "databases.create" | "databases.resize" => {
                let receipt_hash;
                let (project, customer, database) = if action == "databases.create" {
                    let project = text(cmd, "projectID")?;
                    let access = self
                        .authorize_mode(
                            tx,
                            actor,
                            Some(project),
                            None,
                            true,
                            false,
                            authority,
                            true,
                        )
                        .await?;
                    let (hash, prior) = self.replay_control(tx, actor, cmd).await?;
                    if let Some(value) = prior {
                        return Ok(value);
                    }
                    receipt_hash = hash;
                    let row=one(tx,"SELECT jsonb_build_object('id',d.id,'name',d.name) AS value FROM webdock_auth.hosting_database_order o JOIN webdock_auth.hosting_database d ON d.id=o.database_id WHERE o.id=$1 AND o.project_id=$2 AND o.plan_revision_id=$3 AND o.snapshot->>'kind'=$4",vec![text(cmd,"orderID")?.into(),project.into(),text(cmd,"planRevisionID")?.into(),text(cmd,"kind")?.into()]).await?;
                    if row["name"] != cmd["name"] {
                        return Err(conflict("An accepted matching database order is required."));
                    }
                    (
                        project.to_owned(),
                        text(&access, "customerID")?.to_owned(),
                        text(&row, "id")?.to_owned(),
                    )
                } else {
                    let d = self
                        .control_access(
                            tx,
                            actor,
                            authority,
                            text(cmd, "databaseID")?,
                            true,
                            false,
                            true,
                        )
                        .await?;
                    let (hash, prior) = self.replay_control(tx, actor, cmd).await?;
                    if let Some(value) = prior {
                        return Ok(value);
                    }
                    receipt_hash = hash;
                    let order=one(tx,"SELECT snapshot AS value FROM webdock_auth.hosting_database_order WHERE id=$1 AND database_id=$2 AND plan_revision_id=$3",vec![text(cmd,"targetOrderID")?.into(),text(&d,"id")?.into(),text(cmd,"targetPlanRevisionID")?.into()]).await?;
                    if order["sourceRevision"] != cmd["revision"]
                        || ![&d["order_id"], &d["pending_order_id"]]
                            .contains(&&cmd["targetOrderID"])
                    {
                        return Err(conflict(
                            "An accepted matching database upgrade order is required.",
                        ));
                    }
                    (
                        text(&d, "project_id")?.to_owned(),
                        text(&d, "customer_id")?.to_owned(),
                        text(&d, "id")?.to_owned(),
                    )
                };
                let result = self.managed_view(tx, &database).await?;
                self.record_control(
                    tx,
                    actor,
                    cmd,
                    &receipt_hash,
                    &result,
                    ReceiptScope {
                        project: Some(&project),
                        customer: Some(&customer),
                        cluster: None,
                    },
                )
                .await?;
                Ok(result)
            }
            _ => self.maintenance(tx, actor, authority, cmd).await,
        }
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Explicit transaction, authority and revision inputs keep this operation boundary auditable."
    )]
    pub(super) async fn control_access(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        authority: &StoredAuthority,
        id: &str,
        write: bool,
        operator: bool,
        live: bool,
    ) -> Result<Value> {
        let row = one(
            tx,
            "SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database d WHERE id=$1",
            vec![id.into()],
        )
        .await?;
        self.authorize_mode(
            tx,
            actor,
            Some(text(&row, "project_id")?),
            None,
            write,
            operator,
            authority,
            live,
        )
        .await?;
        Ok(row)
    }
    pub(super) async fn managed_view(&self, tx: &DatabaseTransaction, id: &str) -> Result<Value> {
        let row=one(tx,"SELECT to_jsonb(d)||jsonb_build_object('plan',p.plan,'host',b.host,'direct_port',b.direct_port,'pooled_port',b.pooled_port,'endpoint_ready',b.endpoint_ready,'cancellation_requested_at',o.cancellation_requested_at) AS value FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_plan p ON p.id=d.plan_revision_id JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id JOIN webdock_auth.hosting_database_order o ON o.id=d.order_id WHERE d.id=$1",vec![id.into()]).await?;
        crate::management::validate_plan(&row["plan"])?;
        let p = &row["plan"];
        let mut value = json!({"id":row["id"],"projectID":row["project_id"],"name":row["name"],"revision":row["revision"],"observedRevision":row["observed_revision"],"state":row["state"],"planRevisionID":row["plan_revision_id"],"kind":row["kind"],"postgresMajor":p["postgresMajor"],"cpuMillicores":p["cpuMillicores"],"memoryBytes":p["memoryBytes"],"volumeBytes":p["volumeBytes"],"logicalStorageAllowanceBytes":p["logicalStorageAllowanceBytes"],"storageEnforcement":if row["kind"]=="shared"{"measured-logical"}else{"bounded-filesystem"},"databaseName":row["database_name"],"endpoint":if row["endpoint_ready"]==true||(row["state"]=="paused"&&number(&row,"confirmed_credential_revision")?>0){json!({"host":row["host"],"directPort":row["direct_port"],"pooledPort":row["pooled_port"]})}else{Value::Null},"backup":{"status":"not-configured","lastSuccessAt":null,"recoverableUntil":null,"lastRestoreTestAt":null},"operationID":row["operation_id"],"observedAt":iso(&row["observed_at"])? ,"usage":row["usage"],"cancellationRequestedAt":iso(&row["cancellation_requested_at"])?});
        value["backup"] = self.backup_view(tx, id).await?;
        Ok(value)
    }
    pub(super) fn plan_available(&self, row: &Value, customer: &str, operator: bool) -> Result<()> {
        if row["availability"] == "disabled" || (row["availability"] == "pilot" && !operator) {
            return Err(conflict("This database plan is not available."));
        }
        if row["availability"] == "public"
            && !self.management.as_ref().is_some_and(|c| c.public_ordering)
        {
            return Err(conflict("Managed PostgreSQL ordering is not enabled."));
        }
        crate::management::validate_plan(&row["plan"])?;
        if let Some(sponsor) = row["plan"].get("sponsoredCustomerID")
            && (sponsor != customer || row["availability"] != "pilot")
        {
            return Err(Error {
                status: 403,
                message: "This database sponsorship belongs to another customer or is not private.",
            });
        }
        Ok(())
    }
    pub(super) fn protect_database(&self, id: &str, action: &str) -> Result<()> {
        if self
            .management
            .as_ref()
            .and_then(|c| c.protected_database_id.as_deref())
            == Some(id)
            && [
                "pause",
                "restart",
                "resize",
                "restore",
                "delete",
                "purge",
                "rotateCredentials",
                "requestCancellation",
                "upgrade",
            ]
            .contains(&action.trim_start_matches("databases."))
        {
            return Err(conflict(
                "This database hosts the Webdock control plane. Use the reviewed offline maintenance procedure.",
            ));
        }
        Ok(())
    }
    pub(super) async fn replay_control(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        cmd: &Value,
    ) -> Result<(String, Option<Value>)> {
        let hash = hex::encode(Sha256::digest(
            spitzli_hosting::canonical_command(cmd)
                .map_err(|_| invalid())?
                .as_bytes(),
        ));
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            [format!(
                "hosting-request:{}:{}",
                actor.user_id,
                text(cmd, "idempotencyKey")?
            )
            .into()],
        ))
        .await?;
        let row=optional(tx,"SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_operation o WHERE subject=$1 AND idempotency_key=$2",vec![actor.user_id.clone().into(),text(cmd,"idempotencyKey")?.into()]).await?;
        if row.as_ref().is_some_and(|r| r["payload_hash"] != hash) {
            return Err(conflict(
                "Idempotency key already used with different input.",
            ));
        }
        Ok((hash, row.map(|r| r["result"].clone())))
    }
    pub(super) async fn record_control(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        cmd: &Value,
        hash: &str,
        result: &Value,
        scope: ReceiptScope<'_>,
    ) -> Result<()> {
        exec(tx,"INSERT INTO webdock_auth.hosting_operation(id,subject,actor_session,idempotency_key,payload_hash,action,status,result,customer_id,project_id,cluster_id) VALUES($1,$2,$3,$4,$5,$6,'succeeded',$7,$8,$9,$10)",vec![self.id().await?.into(),actor.user_id.clone().into(),actor.session_id.clone().into(),text(cmd,"idempotencyKey")?.into(),hash.into(),text(cmd,"action")?.into(),result.clone().into(),scope.customer.into(),scope.project.into(),scope.cluster.into()]).await
    }
    pub(super) async fn control_audit(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        action: &str,
        target: &str,
    ) -> Result<()> {
        exec(tx,"INSERT INTO webdock_auth.hosting_audit(id,subject,action,target_id,outcome) VALUES($1,$2,$3,$4,'success')",vec![self.id().await?.into(),actor.user_id.clone().into(),action.into(),target.into()]).await
    }
}
pub(super) fn iso(value: &Value) -> Result<Value> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    let value = chrono::DateTime::parse_from_rfc3339(value.as_str().ok_or_else(unavailable)?)
        .map_err(|_| unavailable())?;
    Ok(json!(
        value
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    ))
}

#[cfg(test)]
mod retained_browser_tests {
    use super::super::authority::browser_failure as managed_authentication_error;
    #[test]
    fn managed_database_failure_is_transient_not_definitive_original_authority_loss() {
        let error = managed_authentication_error(spitzli_browser_session::Error::Database(
            sea_orm::DbErr::Custom("isolated transient test only".into()),
        ));
        assert_eq!(error.status, 503);
        assert_eq!(
            managed_authentication_error(spitzli_browser_session::Error::Unauthorized).status,
            404
        );
    }
}
