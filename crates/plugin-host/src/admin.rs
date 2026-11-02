use super::*;
impl HostService {
    pub(super) async fn operator_tx(&self, actor: &Principal) -> Result<DatabaseTransaction> {
        if !actor.is_operator() {
            return denied();
        }
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| PluginError::denied())?;
        timeouts(&tx).await?;
        fence(&tx, true).await?;
        Self::operator_current(&tx).await?;
        Ok(tx)
    }
    async fn operator_current(tx: &DatabaseTransaction) -> Result<()> {
        let row = tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_rls.operator() AS allowed",
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?;
        if !row.try_get::<bool>("", "allowed").map_err(db_error)? {
            return denied();
        }
        Ok(())
    }
    pub(super) async fn audit_admin(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        action: &str,
        target: &str,
    ) -> Result<()> {
        let id = self
            .ids
            .allocate()
            .await
            .map_err(|_| unavailable())?
            .to_string();
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.access_event(id,actor_id,action,target_id,outcome) VALUES($1,$2,$3,$4,'accepted')",[id.into(),actor.user_id.clone().into(),action.into(),target.into()])).await.map_err(db_error)?;
        Ok(())
    }
    pub async fn installed(&self, actor: &Principal) -> Result<Value> {
        self.installed_authority(actor, None).await
    }
    pub async fn installed_with_authority(
        &self,
        actor: &Principal,
        authority: &webdock_browser_session::StoredAuthority,
    ) -> Result<Value> {
        self.installed_authority(actor, Some(authority)).await
    }
    async fn installed_authority(
        &self,
        actor: &Principal,
        authority: Option<&webdock_browser_session::StoredAuthority>,
    ) -> Result<Value> {
        let auth_guard = self.admin_guard(actor, authority, false).await?;
        let tx = self.operator_tx(actor).await?;
        let rows = Self::installed_rows(&tx).await?;
        self.admin_commit(tx, actor, authority, auth_guard, false)
            .await?;
        Ok(Value::Array(rows))
    }
    async fn installed_rows(tx: &DatabaseTransaction) -> Result<Vec<Value>> {
        if tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT singleton FROM webdock_auth.plugin_bootstrap",
            ))
            .await
            .map_err(db_error)?
            .is_none()
        {
            return Err(unsupported("Initialize plugin installation state offline"));
        }
        let rows=tx.query_all_raw(Statement::from_string(DbBackend::Postgres,"SELECT jsonb_build_object('id',id,'kind',kind,'revision',revision::text,'desiredState',desired_state,'state',state,'version',version,'imageDigest',image_digest,'runtime',runtime,'blockers',blockers,'dependencies',dependencies,'managed',EXISTS(SELECT 1 FROM webdock_auth.plugin_install_execution m WHERE m.plugin_id=plugin_installation.id),'installRecovery',(plugin_installation.current_generation IS NULL AND EXISTS(SELECT 1 FROM webdock_auth.plugin_install_execution recovery WHERE recovery.plugin_id=plugin_installation.id))) || jsonb_strip_nulls(jsonb_build_object('operationID',(SELECT o.id FROM webdock_auth.plugin_operation o WHERE o.plugin_id=plugin_installation.id ORDER BY o.created_at DESC,o.id DESC LIMIT 1))) AS value FROM webdock_auth.plugin_installation ORDER BY id")).await.map_err(db_error)?;
        rows.into_iter()
            .map(|row| row.try_get("", "value").map_err(db_error))
            .collect()
    }
    /// Persist the requested transition and operation atomically; a controller owns reconciliation.
    pub async fn request_state(
        &self,
        actor: &Principal,
        id: &str,
        desired: &str,
        expected_revision: &str,
        idempotency_key: &str,
    ) -> Result<Value> {
        self.request_state_authority(actor, id, desired, expected_revision, idempotency_key, None)
            .await
    }
    pub async fn request_state_with_authority(
        &self,
        actor: &Principal,
        authority: &webdock_browser_session::StoredAuthority,
        id: &str,
        desired: &str,
        expected_revision: &str,
        idempotency_key: &str,
    ) -> Result<Value> {
        self.request_state_authority(
            actor,
            id,
            desired,
            expected_revision,
            idempotency_key,
            Some(authority),
        )
        .await
    }
    async fn request_state_authority(
        &self,
        actor: &Principal,
        id: &str,
        desired: &str,
        expected_revision: &str,
        idempotency_key: &str,
        authority: Option<&webdock_browser_session::StoredAuthority>,
    ) -> Result<Value> {
        if !slug(id)
            || !["enabled", "draining", "disabled"].contains(&desired)
            || !crate::data::revision(expected_revision)
            || idempotency_key.is_empty()
            || idempotency_key.encode_utf16().count() > 128
        {
            return Err(invalid("Invalid plugin state command"));
        }
        let auth_guard = self.admin_guard(actor, authority, true).await?;
        let tx = self.operator_tx(actor).await?;
        let payload = json!({"id":id,"desired":desired,"expectedRevision":expected_revision});
        if let Some(prior)=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,status,payload FROM webdock_auth.plugin_operation WHERE actor_id=$1 AND idempotency_key=$2",[actor.user_id.clone().into(),idempotency_key.into()])).await.map_err(db_error)? {
            if {let mut prior_payload=prior.try_get::<Value>("","payload").map_err(db_error)?;if let Some(object)=prior_payload.as_object_mut(){object.remove("_authority");}prior_payload}!=payload{return Err(conflict("Key already used with a different command"));}
            let result=json!({"id":prior.try_get::<String>("","id").map_err(db_error)?,"status":prior.try_get::<String>("","status").map_err(db_error)?});self.admin_commit(tx,actor,authority,auth_guard,true).await?;return Ok(result);
        }
        if tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.plugin_operation WHERE plugin_id=$1 AND status IN ('pending','running','blocked')",[id.into()])).await.map_err(db_error)?.is_some(){return Err(conflict("Resolve the current lifecycle operation first"));}
        let installed = Self::installed_rows(&tx).await?;
        let current = installed
            .iter()
            .find(|plugin| plugin["id"] == id)
            .ok_or_else(|| {
                PluginError::new(
                    webdock_plugin_protocol::ErrorCode::NotFound,
                    "Plugin is not installed",
                )
            })?;
        if current["revision"] != expected_revision {
            return Err(conflict(
                "Installation changed; review its current revision",
            ));
        }
        if current["installRecovery"] == true || tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.plugin_installation WHERE id=$1 AND candidate_generation IS NOT NULL",[id.into()])).await.map_err(db_error)?.is_some() {
            return Err(conflict(
                "Complete or retry initial installation before changing availability",
            ));
        }
        if desired == "enabled" {
            self.require_current_reader(&tx, id).await?;
        }
        if desired != "enabled"
            && installed.iter().any(|p| {
                p["state"] != "disabled"
                    && p["dependencies"]
                        .as_array()
                        .is_some_and(|deps| deps.iter().any(|d| d["id"] == id))
            })
        {
            return Err(conflict("Disable dependent plugins first"));
        }
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_installation SET desired_state=$2,state=CASE WHEN $2 IN ('disabled','draining') AND state<>'disabled' THEN 'draining' ELSE state END,revision=revision+1,generation=generation+1,updated_at=now() WHERE id=$1 AND revision=$3::bigint RETURNING generation",[id.into(),desired.into(),expected_revision.into()])).await.map_err(db_error)?.ok_or_else(||conflict("Installation changed"))?;
        let operation = self
            .ids
            .allocate()
            .await
            .map_err(|_| unavailable())?
            .to_string();
        let mut payload = payload;
        if let Some(authority) = authority {
            payload["_authority"] =
                json!({"subject":actor.user_id,"sessionID":actor.session_id,"proof":authority});
        }
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_operation(id,plugin_id,actor_id,idempotency_key,payload,generation) VALUES($1,$2,$3,$4,$5,$6)",[operation.clone().into(),id.into(),actor.user_id.clone().into(),idempotency_key.into(),payload.into(),row.try_get::<i64>("","generation").map_err(db_error)?.into()])).await.map_err(db_error)?;
        self.audit_admin(&tx, actor, "plugin.state.requested", id)
            .await?;
        self.admin_commit(tx, actor, authority, auth_guard, true)
            .await?;
        Ok(json!({"id":operation,"status":"pending"}))
    }
}

impl HostService {
    pub(super) async fn admin_guard(
        &self,
        actor: &Principal,
        authority: Option<&webdock_browser_session::StoredAuthority>,
        write: bool,
    ) -> Result<Option<DatabaseTransaction>> {
        match authority {
            Some(authority) => Ok(Some(
                self.session_authority
                    .as_ref()
                    .ok_or_else(PluginError::denied)?
                    .begin(
                        &actor.user_id,
                        &actor.session_id,
                        &AuthorityReference::Browser {
                            authority: authority.clone(),
                        },
                        write,
                    )
                    .await?,
            )),
            None => Ok(None),
        }
    }
    pub(super) async fn admin_commit(
        &self,
        tx: DatabaseTransaction,
        actor: &Principal,
        authority: Option<&webdock_browser_session::StoredAuthority>,
        auth: Option<DatabaseTransaction>,
        write: bool,
    ) -> Result<()> {
        Self::operator_current(&tx).await?;
        if let (Some(auth), Some(authority)) = (&auth, authority) {
            self.session_authority
                .as_ref()
                .ok_or_else(PluginError::denied)?
                .verify(
                    auth,
                    &actor.user_id,
                    &actor.session_id,
                    &AuthorityReference::Browser {
                        authority: authority.clone(),
                    },
                    write,
                )
                .await?;
        }
        tx.commit().await.map_err(db_error)?;
        if let Some(auth) = auth {
            auth.commit().await.map_err(db_error)?;
        }
        Ok(())
    }
}
