//! Installation setup for the two compiled, in-process Core capabilities.
use super::*;

/// This whitelist deliberately excludes Mail, which uses signed external installation.
/// Core must select a variant only when its corresponding configured service is loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinModule {
    Hosting,
    Databases,
}

impl BuiltinModule {
    pub fn id(self) -> &'static str {
        match self {
            Self::Hosting => "hosting",
            Self::Databases => "databases",
        }
    }
}

fn activation_payload(module: BuiltinModule, key: &str) -> Result<Value> {
    if key.is_empty() || key.encode_utf16().count() > 128 {
        return Err(invalid("Invalid builtin activation key"));
    }
    Ok(json!({
        "action":"activateBuiltin", "pluginID":module.id(), "bootstrapMode":"fresh",
        "version":env!("CARGO_PKG_VERSION"), "execution":"compiled-in-process"
    }))
}

fn replay(previous: &Value, payload: &Value, receipt: &Value) -> Result<Value> {
    if previous != payload {
        return Err(conflict("Key already used with a different command"));
    }
    Ok(receipt.clone())
}

impl HostService {
    /// Installation scope only: no tenant grants, resources, external runtime or worker dispatch.
    /// The Core caller owns the live service-presence/config check; HTTP supplies only a key.
    pub async fn activate_builtin_with_authority(
        &self,
        actor: &Principal,
        authority: &webdock_browser_session::StoredAuthority,
        module: BuiltinModule,
        idempotency_key: &str,
    ) -> Result<Value> {
        let payload = activation_payload(module, idempotency_key)?;
        if !actor.is_operator() {
            return denied();
        }
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| PluginError::denied())?;
        timeouts(&tx).await?;
        // Match Access preview entry/exit. Never hold native auth row locks while
        // waiting for this business-transaction fence; reload authority afterwards.
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            [format!("native-session-context:{}", actor.session_id).into()],
        ))
        .await
        .map_err(db_error)?;
        let auth = self.admin_guard(actor, Some(authority), true).await?;
        fence(&tx, true).await?;
        tx.query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT 1 WHERE webdock_rls.operator()",
        ))
        .await
        .map_err(db_error)?
        .ok_or_else(PluginError::denied)?;
        // Native identity and the Console mapping must both still grant operator authority.
        // Hold the mapping through commit so a concurrent role change cannot pass a stale replay.
        if tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webdock_admin.users WHERE auth_subject=$1 AND role='operator' FOR SHARE",
            [actor.user_id.clone().into()],
        )).await.map_err(db_error)?.is_none() {
            return Err(PluginError::denied());
        }
        tx.query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock_shared(hashtext('webdock-api-migration'))",
        ))
        .await
        .map_err(db_error)?;
        let marker = tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT mode FROM webdock_auth.plugin_bootstrap WHERE singleton FOR SHARE",
            ))
            .await
            .map_err(db_error)?;
        if marker
            .map(|row| row.try_get::<String>("", "mode").map_err(db_error))
            .transpose()?
            .as_deref()
            != Some("fresh")
        {
            return Err(conflict(
                "Builtin setup requires a fresh native installation",
            ));
        }
        let prior = tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT payload,checkpoint FROM webdock_auth.plugin_operation WHERE actor_id=$1 AND idempotency_key=$2",
            [actor.user_id.clone().into(), idempotency_key.into()],
        )).await.map_err(db_error)?;
        if let Some(prior) = prior {
            let result = replay(
                &prior.try_get::<Value>("", "payload").map_err(db_error)?,
                &payload,
                &prior.try_get::<Value>("", "checkpoint").map_err(db_error)?,
            )?;
            self.admin_commit(tx, actor, Some(authority), auth, true)
                .await?;
            return Ok(result);
        }
        let id = module.id();
        if tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM webdock_auth.plugin_installation WHERE id=$1",
                [id.into()],
            ))
            .await
            .map_err(db_error)?
            .is_some()
        {
            return Err(conflict(
                "Installation already exists; use its reviewed lifecycle",
            ));
        }
        let operation = self
            .ids
            .allocate()
            .await
            .map_err(|_| unavailable())?
            .to_string();
        let receipt = json!({"id":operation,"pluginID":id,"status":"succeeded",
            "kind":"builtin","version":env!("CARGO_PKG_VERSION")});
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO webdock_auth.plugin_installation(id,kind,desired_state,state,version) VALUES($1,'builtin','enabled','enabled',$2)",
            [id.into(), env!("CARGO_PKG_VERSION").into()],
        )).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO webdock_auth.plugin_operation(id,plugin_id,actor_id,idempotency_key,payload,generation,status,phase,checkpoint) VALUES($1,$2,$3,$4,$5,0,'succeeded','activated',$6)",
            [operation.into(), id.into(), actor.user_id.clone().into(), idempotency_key.into(), payload.into(), receipt.clone().into()],
        )).await.map_err(db_error)?;
        self.audit_admin(&tx, actor, "plugin.builtin.activated", id)
            .await?;
        self.admin_commit(tx, actor, Some(authority), auth, true)
            .await?;
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_command_is_fixed_to_compiled_capability_and_validates_key() {
        let payload = activation_payload(BuiltinModule::Hosting, "setup-hosting").unwrap();
        assert_eq!(payload["action"], "activateBuiltin");
        assert_eq!(payload["pluginID"], "hosting");
        assert_eq!(payload["bootstrapMode"], "fresh");
        assert_eq!(payload["version"], env!("CARGO_PKG_VERSION"));
        assert!(activation_payload(BuiltinModule::Hosting, "").is_err());
        assert!(activation_payload(BuiltinModule::Hosting, &"x".repeat(129)).is_err());
        assert!(activation_payload(BuiltinModule::Hosting, &"😀".repeat(65)).is_err());
    }

    #[test]
    fn replay_requires_the_exact_command_and_preserves_receipt() {
        let payload = activation_payload(BuiltinModule::Hosting, "key").unwrap();
        let receipt = json!({"id":"42","status":"succeeded","pluginID":"hosting"});
        assert_eq!(replay(&payload, &payload, &receipt).unwrap(), receipt);
        let databases = activation_payload(BuiltinModule::Databases, "key").unwrap();
        assert!(replay(&payload, &databases, &receipt).is_err());
        let mut changed_version = payload.clone();
        changed_version["version"] = json!("untrusted");
        assert!(replay(&payload, &changed_version, &receipt).is_err());
        assert!(replay(&json!({"action":"state"}), &payload, &receipt).is_err());
    }
}
