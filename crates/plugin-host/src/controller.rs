//! Durable availability transitions for already enrolled external runtimes.
//! Deployment/install/upgrade is a separate operation; disable retains processes and data.
use super::*;
use webdock_plugin_protocol::Availability;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StateRequest {
    id: String,
    desired: Availability,
    expected_revision: String,
    #[serde(rename = "_authority")]
    authority: OperatorProof,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorProof {
    subject: String,
    #[serde(rename = "sessionID")]
    session_id: String,
    proof: webdock_browser_session::StoredAuthority,
}
pub struct StateLease {
    id: String,
    plugin: String,
    actor: String,
    token: uuid::Uuid,
    generation: i64,
    payload: Value,
}
impl StateLease {
    pub fn id(&self) -> &str {
        &self.id
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationSnapshot {
    pub kind: &'static str,
    pub id: String,
    #[serde(rename = "pluginID")]
    pub plugin_id: String,
    pub status: OperationStatus,
    pub phase: String,
    pub generation: String,
    pub cursor: String,
    pub condition: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Blocked,
}
impl OperationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
        }
    }
    fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "blocked" => Ok(Self::Blocked),
            _ => Err(unavailable()),
        }
    }
}
pub struct RuntimeDescription {
    pub manifest: Vec<u8>,
    pub plugin_id: String,
    pub version: String,
    pub generation: u64,
    pub protocol_version: u32,
    pub peer: PeerIdentity,
}
#[async_trait]
pub trait StateProbe: Send + Sync {
    async fn describe(&self, plugin: &str, generation: u64) -> Result<RuntimeDescription>;
}
fn snapshot(row: &sea_orm::QueryResult) -> Result<OperationSnapshot> {
    let checkpoint: Value = row.try_get("", "checkpoint").map_err(db_error)?;
    Ok(OperationSnapshot {
        kind: match row
            .try_get::<Option<String>>("", "kind")
            .ok()
            .flatten()
            .as_deref()
        {
            Some("install" | "installRetry") => "install",
            Some("update" | "updateRetry") => "update",
            Some("rollback" | "rollbackRetry") => "rollback",
            _ => "state",
        },
        id: row.try_get("", "id").map_err(db_error)?,
        plugin_id: row.try_get("", "plugin_id").map_err(db_error)?,
        status: OperationStatus::parse(&row.try_get::<String>("", "status").map_err(db_error)?)?,
        phase: row.try_get("", "phase").map_err(db_error)?,
        generation: row
            .try_get::<i64>("", "generation")
            .map_err(db_error)?
            .to_string(),
        cursor: checkpoint["sequence"].as_u64().unwrap_or(0).to_string(),
        condition: checkpoint["condition"].as_str().map(str::to_owned),
    })
}
impl HostService {
    pub async fn operation_with_authority(
        &self,
        actor: &Principal,
        authority: &webdock_browser_session::StoredAuthority,
        id: &str,
    ) -> Result<OperationSnapshot> {
        if !crate::data::revision(id) {
            return Err(invalid("Invalid operation"));
        }
        let auth = self.admin_guard(actor, Some(authority), false).await?;
        let tx = self.operator_tx(actor).await?;
        let row = tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT id,plugin_id,status,phase,generation,checkpoint,payload->>'kind' AS kind FROM webdock_auth.plugin_operation WHERE id=$1",
            [id.into()])).await.map_err(db_error)?.ok_or_else(|| PluginError::new(webdock_plugin_protocol::ErrorCode::NotFound,"Operation not found"))?;
        let result = snapshot(&row)?;
        self.admin_commit(tx, actor, Some(authority), auth, false)
            .await?;
        Ok(result)
    }

    /// A crash leaves a bounded lease. A successor gets a fresh token; old completion cannot commit.
    pub async fn claim_state_operation(&self, plugin: Option<&str>) -> Result<Option<StateLease>> {
        if plugin.is_some_and(|value| !slug(value)) {
            return Err(invalid("Invalid plugin"));
        }
        let tx = self.controller().await?;
        fence(&tx, true).await?;
        let row = tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT id,plugin_id,actor_id,generation,payload FROM webdock_auth.plugin_operation WHERE status IN ('pending','running') AND available_at<=now() AND (lease_until IS NULL OR lease_until<=now()) AND payload ? 'desired' AND ($1::text IS NULL OR plugin_id=$1) ORDER BY available_at,id FOR UPDATE SKIP LOCKED LIMIT 1",[plugin.into()]))
            .await.map_err(db_error)?;
        let Some(row) = row else {
            tx.commit().await.map_err(db_error)?;
            return Ok(None);
        };
        let token = uuid::Uuid::new_v4();
        let lease = StateLease {
            id: row.try_get("", "id").map_err(db_error)?,
            plugin: row.try_get("", "plugin_id").map_err(db_error)?,
            actor: row.try_get("", "actor_id").map_err(db_error)?,
            generation: row.try_get("", "generation").map_err(db_error)?,
            payload: row.try_get("", "payload").map_err(db_error)?,
            token,
        };
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "UPDATE webdock_auth.plugin_operation SET status='running',phase='checking',claim_token=$2::uuid,lease_until=now()+interval '15 seconds',attempts=attempts+1,checkpoint=jsonb_build_object('sequence',coalesce((checkpoint->>'sequence')::bigint,0)+1),updated_at=now() WHERE id=$1",
            [lease.id.clone().into(),lease.token.to_string().into()])).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(Some(lease))
    }
    async fn state_lock(&self, tx: &DatabaseTransaction, lease: &StateLease) -> Result<()> {
        fence(tx, true).await?;
        let valid = tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT o.id FROM webdock_auth.plugin_operation o JOIN webdock_auth.plugin_installation p ON p.id=o.plugin_id WHERE o.id=$1 AND o.plugin_id=$2 AND o.generation=$3 AND p.generation=o.generation AND o.payload=$5 AND o.status='running' AND o.claim_token=$4::uuid AND o.lease_until>now() AND p.desired_state=o.payload->>'desired' FOR UPDATE OF p,o",
            [lease.id.clone().into(),lease.plugin.clone().into(),lease.generation.into(),lease.token.to_string().into(),lease.payload.clone().into()])).await.map_err(db_error)?;
        if valid.is_none() {
            return denied();
        }
        Ok(())
    }
    async fn operator_proof(
        &self,
        lease: &StateLease,
    ) -> Result<(StateRequest, DatabaseTransaction)> {
        let request: StateRequest =
            serde_json::from_value(lease.payload.clone()).map_err(|_| PluginError::denied())?;
        if request.id != lease.plugin
            || request.authority.subject != lease.actor
            || !crate::data::revision(&request.expected_revision)
        {
            return denied();
        }
        let auth = self
            .session_authority
            .as_ref()
            .ok_or_else(PluginError::denied)?
            .begin_operator(
                &request.authority.subject,
                &request.authority.session_id,
                &request.authority.proof,
            )
            .await?;
        Ok((request, auth))
    }
    async fn state_finish(
        &self,
        tx: DatabaseTransaction,
        lease: &StateLease,
        status: OperationStatus,
        phase: &str,
        condition: Option<&str>,
    ) -> Result<OperationSnapshot> {
        let row = tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "UPDATE webdock_auth.plugin_operation SET status=$3,phase=$4,claim_token=NULL,lease_until=NULL,available_at=now()+interval '1 second',checkpoint=jsonb_build_object('sequence',coalesce((checkpoint->>'sequence')::bigint,0)+1,'condition',$5::text),updated_at=now() WHERE id=$1 AND claim_token=$2::uuid AND status='running' AND lease_until>now() RETURNING id,plugin_id,status,phase,generation,checkpoint",
            [lease.id.clone().into(),lease.token.to_string().into(),status.as_str().into(),phase.into(),condition.into()])).await.map_err(db_error)?.ok_or_else(PluginError::denied)?;
        let result = snapshot(&row)?;
        tx.commit().await.map_err(db_error)?;
        Ok(result)
    }
    async fn state_failure(
        &self,
        lease: &StateLease,
        condition: &str,
    ) -> Result<OperationSnapshot> {
        let tx = self.controller().await?;
        fence(&tx, true).await?;
        self.state_finish(
            tx,
            lease,
            OperationStatus::Failed,
            "failed",
            Some(condition),
        )
        .await
    }
    pub async fn advance_state_operation(
        &self,
        lease: &StateLease,
        probe: &dyn StateProbe,
    ) -> Result<OperationSnapshot> {
        match self.advance_state_inner(lease, probe).await {
            Ok(result) => Ok(result),
            Err(error) => {
                use webdock_plugin_protocol::ErrorCode;
                if matches!(
                    error.code,
                    ErrorCode::Unavailable | ErrorCode::DeadlineExceeded
                ) {
                    let tx = self.controller().await?;
                    self.state_lock(&tx, lease).await?;
                    let attempts: i32 = tx
                        .query_one_raw(Statement::from_sql_and_values(
                            DbBackend::Postgres,
                            "SELECT attempts FROM webdock_auth.plugin_operation WHERE id=$1",
                            [lease.id.clone().into()],
                        ))
                        .await
                        .map_err(db_error)?
                        .ok_or_else(unavailable)?
                        .try_get("", "attempts")
                        .map_err(db_error)?;
                    return self
                        .state_finish(
                            tx,
                            lease,
                            if attempts < 5 {
                                OperationStatus::Pending
                            } else {
                                OperationStatus::Failed
                            },
                            "runtime_unavailable",
                            Some("runtime_unavailable"),
                        )
                        .await;
                }
                self.state_failure(
                    lease,
                    if error.code == ErrorCode::PermissionDenied {
                        "authority_changed"
                    } else {
                        "unsupported_transition"
                    },
                )
                .await
            }
        }
    }
    async fn advance_state_inner(
        &self,
        lease: &StateLease,
        probe: &dyn StateProbe,
    ) -> Result<OperationSnapshot> {
        let (request, auth) = match self.operator_proof(lease).await {
            Ok(proof) => proof,
            Err(error) if error.code == webdock_plugin_protocol::ErrorCode::PermissionDenied => {
                return self.state_failure(lease, "authority_revoked").await;
            }
            Err(error) => return Err(error),
        };
        let tx = self.controller().await?;
        self.state_lock(&tx, lease).await?;
        let baseline = if request.desired == Availability::Enabled {
            let metadata = self.metadata(&tx, &lease.plugin, None).await?;
            metadata.manifest.require_action_only_lifecycle()?;
            self.require_current_reader(&tx, &lease.plugin).await?;
            let binding = self.binding(&lease.plugin, &metadata).await?;
            Some((
                runtime_fingerprint(&metadata, &binding)?,
                binding.generation,
            ))
        } else {
            None
        };
        tx.commit().await.map_err(db_error)?;
        auth.commit().await.map_err(db_error)?;
        let description = match &baseline {
            Some((_, generation)) => Some(
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    probe.describe(&lease.plugin, *generation),
                )
                .await
                .map_err(|_| unavailable())??,
            ),
            None => None,
        };
        let (_, auth) = match self.operator_proof(lease).await {
            Ok(proof) => proof,
            Err(error) if error.code == webdock_plugin_protocol::ErrorCode::PermissionDenied => {
                return self.state_failure(lease, "authority_revoked").await;
            }
            Err(error) => return Err(error),
        };
        let tx = self.controller().await?;
        self.state_lock(&tx, lease).await?;
        if let (Some((fingerprint, _)), Some(proof)) = (baseline, description) {
            self.require_current_reader(&tx, &lease.plugin).await?;
            let fresh = self.metadata(&tx, &lease.plugin, None).await?;
            let binding = self.binding(&lease.plugin, &fresh).await?;
            if fingerprint != runtime_fingerprint(&fresh, &binding)? {
                return denied();
            }
            if proof.manifest != fresh.manifest.as_bytes()
                || proof.plugin_id != lease.plugin
                || proof.version != fresh.manifest.version()
                || proof.generation != binding.generation
                || proof.protocol_version != 1
                || !binding.permits(&proof.peer)
            {
                return self
                    .state_finish(
                        tx,
                        lease,
                        OperationStatus::Failed,
                        "failed",
                        Some("runtime_mismatch"),
                    )
                    .await;
            }
        }
        let draining = if request.desired == Availability::Disabled {
            tx.query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.plugin_state_continuations($1,$2::uuid) AS active",
                [lease.id.clone().into(), lease.token.to_string().into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(unavailable)?
            .try_get::<Option<bool>>("", "active")
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?
        } else {
            false
        };
        let state = if draining {
            "draining"
        } else {
            request.desired.as_str()
        };
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "UPDATE webdock_auth.plugin_installation SET state=$2,blockers=$3,revision=revision+1,updated_at=now() WHERE id=$1",
            [lease.plugin.clone().into(),state.into(),if draining { json!([{"code":"active_continuations","message":"Waiting for accepted work"}]) } else { json!([]) }.into()])).await.map_err(db_error)?;
        let result = self
            .state_finish(
                tx,
                lease,
                if draining {
                    OperationStatus::Pending
                } else {
                    OperationStatus::Succeeded
                },
                if draining { "draining" } else { "complete" },
                draining.then_some("active_continuations"),
            )
            .await?;
        auth.commit().await.map_err(db_error)?;
        Ok(result)
    }
}

fn runtime_fingerprint(metadata: &Metadata, binding: &RuntimeBinding) -> Result<String> {
    Ok(digest(
        serde_json::to_vec(&json!({
            "installation":metadata.installation,"runtime":metadata.runtime,
            "configuration":metadata.configuration,"release":metadata.release,"binding":binding
        }))
        .map_err(|_| unavailable())?,
    ))
}
