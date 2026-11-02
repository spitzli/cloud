//! Neutral transaction helpers. The Mail domain owns ledger state and never delegates it to Host.
use super::*;
#[derive(Clone, Copy)]
pub enum MailRuntimeMode {
    Admission,
    Dispatch,
    Bookkeeping,
}
impl MailRuntimeMode {
    pub fn permits(self, state: &str) -> bool {
        match self {
            Self::Admission => state == "enabled",
            Self::Dispatch => ["enabled", "draining"].contains(&state),
            Self::Bookkeeping => ["enabled", "draining", "disabled"].contains(&state),
        }
    }
}
pub struct MailRuntimeProof {
    pub generation: u64,
    pub credential_sha256: String,
    pub manifest_sha256: String,
    pub configuration_revision: u64,
    pub state: String,
}
/// Holds canonical Browser/MCP authority before any worker/business lock is acquired.
pub struct MailActionGuard {
    auth: DatabaseTransaction,
    authority: AuthorityReference,
    invocation: PersistedInvocation,
    pub claims: InvocationClaims,
}
pub(super) fn readable_custody(metadata: &Metadata) -> Result<()> {
    if metadata.installation["schema_high_water"].is_null() {
        return Ok(());
    }
    let high = integer(&metadata.installation, "schema_high_water")?;
    let readable = &metadata.manifest.as_json()["readableSchema"];
    let min = readable["min"].as_i64().ok_or_else(unavailable)?;
    let max = readable["max"].as_i64().ok_or_else(unavailable)?;
    if high > max || high > 0 && high < min {
        return denied();
    }
    Ok(())
}
impl HostService {
    pub(super) async fn mail_binding_in(
        &self,
        metadata: &Metadata,
        enrollment: Option<Value>,
    ) -> Result<RuntimeBinding> {
        let binding = self
            .bindings
            .runtime_from_enrollment("mail", enrollment)
            .await?
            .ok_or_else(PluginError::denied)?;
        self.checked_binding(metadata, binding)
    }
    pub async fn mail_runtime_in(
        &self,
        tx: &DatabaseTransaction,
        peer: &PeerIdentity,
        generation: u64,
        mode: MailRuntimeMode,
    ) -> Result<MailRuntimeProof> {
        if !self.native_mail_control || generation == 0 || generation > i64::MAX as u64 {
            return denied();
        }
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.managed_mail_runtime($1,$2) AS proof",
                [
                    (generation as i64).into(),
                    matches!(mode, MailRuntimeMode::Bookkeeping).into(),
                ],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(unavailable)?;
        let proof: Value = row
            .try_get::<Option<Value>>("", "proof")
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?;
        let manifest = Manifest::parse(string(&proof["release"], "manifest_bytes")?.as_bytes())?;
        let metadata = Metadata {
            manifest,
            release: proof["release"].clone(),
            installation: proof["installation"].clone(),
            runtime: proof["runtime"].clone(),
            configuration: proof["configuration"].clone(),
            grant: Value::Null,
        };
        readable_custody(&metadata)?;
        if metadata.manifest.id() != "mail"
            || metadata.manifest.version() != string(&metadata.installation, "version")?
            || metadata.manifest.as_json()["image"] != metadata.release["image"]
        {
            return denied();
        }
        let state = string(&metadata.installation, "state")?;
        if !mode.permits(state) {
            return denied();
        }
        let binding = self
            .mail_binding_in(
                &metadata,
                (proof["managedEnrollment"] == true).then(|| proof["enrollment"].clone()),
            )
            .await?;
        if !binding.permits(peer) || binding.generation != generation {
            return denied();
        }
        Ok(MailRuntimeProof {
            generation,
            credential_sha256: binding.credential_sha256,
            manifest_sha256: string(&metadata.release, "manifest_sha256")?.into(),
            configuration_revision: integer(&metadata.configuration, "revision")? as u64,
            state: state.into(),
        })
    }
    pub async fn begin_mail_action(&self, token: &str) -> Result<MailActionGuard> {
        if token.len() > 16384 {
            return denied();
        }
        let body = token.split_once('.').ok_or_else(PluginError::denied)?.0;
        let hint: InvocationClaims = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(body)
                .map_err(|_| PluginError::denied())?,
        )
        .map_err(|_| PluginError::denied())?;
        let claims = verify_invocation(
            token,
            &self.key.verifying_key(),
            "mail",
            hint.generation,
            chrono::Utc::now().timestamp(),
        )?;
        if claims.job.is_some()
            || ![
                "mail.send",
                "mail.status",
                "mail.cancel",
                "mail.history",
                "incoming.activate",
                "mailboxes.create",
                "domains.delete",
            ]
            .contains(&claims.action.as_str())
        {
            return denied();
        }
        // Discovery releases its worker connection before acquiring canonical auth.
        let discovery = self.controller().await?;
        let row=discovery.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(v) AS invocation FROM webdock_auth.plugin_invocation v WHERE id=$1::uuid AND revoked_at IS NULL AND expires_at>clock_timestamp()",[claims.id.clone().into()])).await.map_err(db_error)?.ok_or_else(PluginError::denied)?;
        let row: Value = row.try_get("", "invocation").map_err(db_error)?;
        discovery.commit().await.map_err(db_error)?;
        let authority: AuthorityReference =
            serde_json::from_value(row["session_context"]["authority"].clone())
                .map_err(|_| PluginError::denied())?;
        let invocation = PersistedInvocation {
            id: claims.id.clone(),
            actor: string(&row, "actor_id")?.into(),
            session: string(&row, "session_id")?.into(),
            row,
        };
        let auth = self
            .session_authority
            .as_ref()
            .ok_or_else(PluginError::denied)?
            .begin(
                &invocation.actor,
                &invocation.session,
                &authority,
                claims.action != "mail.status" && claims.action != "mail.history",
            )
            .await?;
        Ok(MailActionGuard {
            auth,
            authority,
            invocation,
            claims,
        })
    }
    /// Re-read original admission under the same worker transaction used for the send ledger.
    pub async fn mail_action_in(
        &self,
        tx: &DatabaseTransaction,
        guard: &MailActionGuard,
        input: &Value,
    ) -> Result<Value> {
        if crate::data::javascript_json(input, false)?.len()
            > invocation_input_limit(
                &guard.claims.plugin_id,
                &guard.claims.action,
                guard.claims.job.is_some(),
            )
        {
            return Err(invalid("Mail action input exceeds limit"));
        }
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.managed_mail_action($1::uuid,$2) AS proof",
                [
                    guard.claims.id.clone().into(),
                    (if guard.claims.action == "mailboxes.create" {
                        input["domain_id"].as_str()
                    } else {
                        input["resource_id"].as_str()
                    })
                    .into(),
                ],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(unavailable)?;
        let proof: Value = row
            .try_get::<Option<Value>>("", "proof")
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?;
        let inv = &proof["invocation"];
        let claims = &guard.claims;
        if inv != &guard.invocation.row
            || inv["plugin_id"] != "mail"
            || inv["runtime_generation"] != claims.generation
            || inv["project_id"] != claims.project_id
            || inv["customer_id"] != claims.customer_id
            || inv["name"] != claims.action
            || inv["request_hash"] != digest(crate::data::javascript_json(input, false)?)
            || (claims.action != "mailboxes.create"
                && inv["session_context"]["resourceID"] != input["resource_id"])
        {
            return denied();
        }
        // Pin the effective configuration and enrollment exactly as at canonical admission.
        let runtime = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.managed_mail_runtime($1,false) AS proof",
                [(claims.generation as i64).into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(unavailable)?;
        let runtime: Value = runtime
            .try_get::<Option<Value>>("", "proof")
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?;
        let metadata = Metadata {
            manifest: Manifest::parse(string(&runtime["release"], "manifest_bytes")?.as_bytes())?,
            release: runtime["release"].clone(),
            installation: runtime["installation"].clone(),
            runtime: runtime["runtime"].clone(),
            configuration: runtime["configuration"].clone(),
            grant: json!({"customer_id":inv["customer_id"],"revision":inv["grant_revision"],"revoked_at":null,"capabilities":inv["capabilities"]}),
        };
        let binding = self
            .mail_binding_in(
                &metadata,
                (runtime["managedEnrollment"] == true).then(|| runtime["enrollment"].clone()),
            )
            .await?;
        let configuration = Self::current_configuration(&metadata, &binding, &claims.project_id)?;
        if inv["session_context"]["nativeAdmission"]
            != Self::admission_snapshot(&metadata, &binding, &configuration)?
        {
            return denied();
        }
        Ok(proof)
    }
    pub async fn verify_mail_action(&self, guard: &MailActionGuard) -> Result<()> {
        self.session_authority
            .as_ref()
            .ok_or_else(PluginError::denied)?
            .verify(
                &guard.auth,
                &guard.invocation.actor,
                &guard.invocation.session,
                &guard.authority,
                guard.claims.action != "mail.status" && guard.claims.action != "mail.history",
            )
            .await
    }
    pub async fn complete_mail_action(&self, guard: MailActionGuard) -> Result<()> {
        guard.auth.commit().await.map_err(db_error)
    }
    pub async fn mail_source_in(&self, tx: &DatabaseTransaction, source: &Value) -> Result<bool> {
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.managed_mail_source($1) AS allowed",
            [source.clone().into()],
        ))
        .await
        .map_err(db_error)?
        .ok_or_else(unavailable)?
        .try_get("", "allowed")
        .map_err(db_error)
    }
}
