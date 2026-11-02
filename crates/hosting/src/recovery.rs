//! Operator-only standing recovery policy on existing native operation/agent authority.
//! Intentionally unlinked until the coordinated compiler/schema hold is released.
use super::*;
use serde::Deserialize;
use webdock_agent_protocol::recovery::{
    RecoveryAction, RecoveryClaim, RecoveryCompletion, RecoveryLease, RecoveryProfile, RecoverySpec,
};
use webdock_browser_session::StoredAuthority;

#[derive(Deserialize)]
#[serde(tag = "action", deny_unknown_fields)]
enum RecoveryCommand {
    #[serde(rename = "recovery.policies.grant", rename_all = "camelCase")]
    Grant {
        #[serde(rename = "clusterID")]
        cluster_id: String,
        profile: RecoveryProfile,
        #[serde(rename = "sourceAppID")]
        source_app_id: Option<String>,
        profile_id: String,
        capacity_bytes: u64,
        interval_seconds: u32,
        idempotency_key: String,
    },
    #[serde(rename = "recovery.policies.revoke", rename_all = "camelCase")]
    Revoke {
        #[serde(rename = "policyID")]
        policy_id: String,
        revision: u64,
        idempotency_key: String,
    },
    #[serde(rename = "recovery.backup", rename_all = "camelCase")]
    Backup {
        #[serde(rename = "policyID")]
        policy_id: String,
        revision: u64,
        idempotency_key: String,
    },
    #[serde(rename = "recovery.restore", rename_all = "camelCase")]
    Restore {
        #[serde(rename = "recoverySetID")]
        recovery_set_id: String,
        target: Value,
        idempotency_key: String,
    },
    #[serde(rename = "recovery.status", rename_all = "camelCase")]
    Status {
        #[serde(rename = "policyID")]
        policy_id: String,
    },
    #[serde(rename = "recovery.sets.export", rename_all = "camelCase")]
    Export {
        #[serde(rename = "recoverySetID")]
        recovery_set_id: String,
    },
}
fn failure() -> crate::HostingError {
    error(
        409,
        "Current recovery policy, resource and enrolled agent are required.",
    )
}
fn object(row: sea_orm::QueryResult) -> Result<Value> {
    row.try_get("", "value").map_err(db_error)
}
fn native_recovery(row: sea_orm::QueryResult) -> Result<Value> {
    native_recovery_value(row.try_get::<Option<Value>>("", "value"))
}
fn native_recovery_value(
    value: std::result::Result<Option<Value>, sea_orm::DbErr>,
) -> Result<Value> {
    value
        .map_err(db_error)?
        .filter(|value| !value.is_null())
        .ok_or_else(failure)
}
fn profile_name(profile: RecoveryProfile) -> Result<String> {
    serde_json::to_value(profile)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(failure)
}
fn rand_bytes() -> [u8; 32] {
    let mut b = [0; 32];
    getrandom::getrandom(&mut b).expect("operating system randomness");
    b
}
fn digest(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

const MAIL_INVENTORY_SQL: &str = "SELECT webdock_rls.recovery_mail_inventory($1,$2,$3,$4) AS value";
fn inventory_parameters(
    operation: &str,
    generation: u64,
    token_hash: &str,
    agent_generation: i32,
) -> Vec<SqlValue> {
    vec![
        operation.into(),
        (generation as i32).into(),
        token_hash.into(),
        agent_generation.into(),
    ]
}
#[path = "recovery_app.rs"]
mod app_source;
#[path = "recovery_reservations.rs"]
mod reservations;
impl HostingService {
    pub fn with_recovery_signer(mut self, signer: crate::RecoverySigner) -> Self {
        self.recovery_signer = Some(signer);
        self
    }
    pub fn with_recovery_signer_configuration(
        mut self,
        key: Option<std::path::PathBuf>,
        directory: Option<std::path::PathBuf>,
    ) -> Result<Self> {
        match (key, directory) {
            (None, None) => {}
            (Some(key), Some(directory)) => {
                self.recovery_signer = Some(crate::RecoverySigner::new(key, directory)?)
            }
            _ => {
                return Err(error(
                    503,
                    "Both independent recovery signer paths are required.",
                ));
            }
        }
        Ok(self)
    }
    /// Browser activation authorizes the durable policy once. Recurrence never creates
    /// or impersonates a session; current operator/resource entitlement remains mandatory.
    pub async fn recovery_command(
        &self,
        actor: &Principal,
        authority: &StoredAuthority,
        input: Value,
    ) -> Result<Value> {
        if actor.in_read_only_preview() || authority.read_only() {
            return Err(error(403, "Recovery requires platform operator authority."));
        }
        let proof = serde_json::to_value(authority).map_err(|_| failure())?;
        let browser = self
            .browser_guard(&actor.user_id, &actor.session_id, &proof)
            .await?;
        let tx = self.begin(actor).await?;
        Self::module(&tx, true, false).await?;
        let native_actor = HostingActor::from(actor);
        Self::authorize(&tx, &native_actor, None, None, true, true, true).await?;
        let command: RecoveryCommand = serde_json::from_value(input.clone())
            .map_err(|_| error(400, "Invalid recovery command."))?;
        if let RecoveryCommand::Export { recovery_set_id } = &command {
            contracts::record_id(recovery_set_id)?;
            let row=one(&tx,"SELECT to_jsonb(s)||jsonb_build_object('profile',p.profile,'profileID',p.source->>'profileID','approvedBy',p.approved_by,'capacityBytes',p.capacity_bytes) AS value FROM webdock_auth.hosting_recovery_set s JOIN webdock_auth.hosting_recovery_policy p ON p.id=s.policy_id JOIN webdock_auth.hosting_operation o ON o.id=s.operation_id WHERE s.id=$1 AND s.referenced AND o.status='succeeded'",vec![recovery_set_id.clone().into()]).await?.map(object).transpose()?.ok_or_else(failure)?;
            let admitted = webdock_agent_protocol::recovery::custody::AdmittedRecoverySet {
                version: 1,
                set_id: field(&row, "id")?.into(),
                operation_id: field(&row, "operation_id")?.into(),
                policy_id: field(&row, "policy_id")?.into(),
                policy_revision: numeric(&row, "policy_revision")? as u64,
                profile: serde_json::from_value(row["profile"].clone()).map_err(|_| failure())?,
                profile_id: field(&row, "profileID")?.into(),
                approved_by: field(&row, "approvedBy")?.into(),
                epoch: field(&row, "epoch")?.into(),
                capacity_bytes: numeric(&row, "capacityBytes")? as u64,
                proof: row["proof"].clone(),
                completed_at: epoch(&row["completed_at"]).ok_or_else(failure)?,
            };
            browser.verify().await?;
            let signed = self
                .recovery_signer
                .as_ref()
                .ok_or_else(|| {
                    error(
                        503,
                        "Independent operator recovery signer is not configured.",
                    )
                })?
                .sign(&admitted)?;
            self.audit(
                &tx,
                &actor.user_id,
                "recovery.sets.export",
                recovery_set_id,
                "issued",
            )
            .await?;
            tx.commit().await.map_err(db_error)?;
            return serde_json::to_value(signed).map_err(|_| failure());
        }
        if let RecoveryCommand::Status { policy_id } = &command {
            contracts::record_id(policy_id)?;
            let policy=one(&tx,"SELECT jsonb_build_object('id',p.id,'profile',p.profile,'revision',p.revision,'status',p.observed_status,'revokedAt',p.revoked_at,'lastSuccessAt',p.last_success_at,'lastError',p.last_error,'nextRunAt',p.next_run_at,'productionActivation',false) AS value FROM webdock_auth.hosting_recovery_policy p WHERE p.id=$1",vec![policy_id.clone().into()]).await?.map(object).transpose()?.ok_or_else(failure)?;
            let sets=rows(&tx,"SELECT jsonb_build_object('id',id,'operationID',operation_id,'completedAt',completed_at) AS value FROM webdock_auth.hosting_recovery_set WHERE policy_id=$1 ORDER BY completed_at DESC LIMIT 100",vec![policy_id.clone().into()]).await?;
            browser.verify().await?;
            tx.commit().await.map_err(db_error)?;
            return Ok(json!({"policy":policy,"sets":sets}));
        }
        let (RecoveryCommand::Grant {
            idempotency_key: key,
            ..
        }
        | RecoveryCommand::Revoke {
            idempotency_key: key,
            ..
        }
        | RecoveryCommand::Backup {
            idempotency_key: key,
            ..
        }
        | RecoveryCommand::Restore {
            idempotency_key: key,
            ..
        }) = &command
        else {
            unreachable!()
        };
        if key.is_empty() || key.len() > 128 || key.chars().any(char::is_control) {
            return Err(error(400, "Invalid idempotency key."));
        }
        let (request_hash, prior) = self.replay(&tx, &native_actor, key, &input).await?;
        if let Some(prior) = prior {
            browser.verify().await?;
            tx.commit().await.map_err(db_error)?;
            return Ok(prior["result"].clone());
        }
        let result = match command {
            RecoveryCommand::Grant {
                cluster_id,
                profile,
                source_app_id,
                profile_id,
                capacity_bytes,
                interval_seconds,
                ..
            } => {
                contracts::record_id(&cluster_id)?;
                if !(60..=2592000).contains(&interval_seconds)
                    || !(67108864..=64 * 1024 * 1024 * 1024).contains(&capacity_bytes)
                {
                    return Err(error(
                        400,
                        "Recovery capacity or schedule is outside the supported bounds.",
                    ));
                }
                let agent=one(&tx,"SELECT a.observation->'capabilities'->'nativeRecovery' AS value FROM webdock_auth.hosting_agent a JOIN webdock_auth.hosting_cluster c ON c.id=a.cluster_id WHERE c.id=$1 AND c.ownership='platform' AND c.verified AND NOT a.revoked AND a.last_seen>clock_timestamp()-interval '90 seconds' FOR SHARE OF a,c",vec![cluster_id.clone().into()]).await?.map(native_recovery).transpose()?.ok_or_else(failure)?;
                let destination = agent["profiles"]
                    .as_array()
                    .and_then(|rows| {
                        rows.iter().find(|v| {
                            v["id"] == profile_id
                                && v["profile"] == profile_name(profile).unwrap_or_default()
                        })
                    })
                    .cloned()
                    .ok_or_else(failure)?;
                if agent["version"] != 2
                    || destination["keyCustodyID"].as_str().is_none()
                    || destination["sha256"].as_str().is_none_or(|v| v.len() != 64)
                {
                    return Err(failure());
                }
                let source = if profile == RecoveryProfile::MailInstallationV1 {
                    if source_app_id.is_some() {
                        return Err(error(400, "Mail recovery is installation-wide."));
                    }
                    one(&tx,"SELECT jsonb_build_object('mailGeneration',generation,'mailRevision',revision,'mailVersion',version,'profileID',$1::text) AS value FROM webdock_auth.plugin_installation WHERE id='mail' AND state='enabled' FOR SHARE",vec![profile_id.clone().into()]).await?.map(object).transpose()?.ok_or_else(failure)?
                } else {
                    let app = source_app_id.as_deref().ok_or_else(failure)?;
                    contracts::record_id(app)?;
                    let source=one(&tx,"SELECT jsonb_build_object('appID',a.id,'projectID',a.project_id,'customerID',h.customer_id,'revision',a.revision,'runtimeUID',a.observed_uid,'spec',a.spec,'profileID',$2::text) AS value FROM webdock_auth.hosting_app a JOIN webdock_auth.hosting_project h ON h.project_id=a.project_id WHERE a.id=$1 AND h.cluster_id=$3 AND a.status='ready' AND a.lifecycle='deployed' AND a.revision=a.observed_revision AND a.observed_uid IS NOT NULL FOR SHARE OF a,h",vec![app.into(),profile_id.clone().into(),cluster_id.clone().into()]).await?.map(object).transpose()?.ok_or_else(failure)?;
                    Self::authorize(
                        &tx,
                        &native_actor,
                        source["projectID"].as_str(),
                        None,
                        true,
                        true,
                        true,
                    )
                    .await?;
                    source
                };
                let id = self.id().await?;
                execute(&tx,"INSERT INTO webdock_auth.hosting_recovery_policy(id,cluster_id,profile,source_app_id,source,destination,capacity_bytes,interval_seconds,approved_by,approved_session) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",vec![id.clone().into(),cluster_id.into(),profile_name(profile)?.into(),source_app_id.into(),source.into(),destination.into(),(capacity_bytes as i64).into(),(interval_seconds as i32).into(),actor.user_id.clone().into(),actor.session_id.clone().into()]).await?;
                json!({"policyID":id,"revision":1,"status":"configured","productionActivation":false})
            }
            RecoveryCommand::Revoke {
                policy_id,
                revision,
                ..
            } => {
                contracts::record_id(&policy_id)?;
                let n=execute(&tx,"UPDATE webdock_auth.hosting_recovery_policy SET revoked_at=clock_timestamp(),revision=revision+1,observed_status='revoked' WHERE id=$1 AND revision=$2 AND revoked_at IS NULL",vec![policy_id.clone().into(),(revision as i64).into()]).await?;
                if n != 1 {
                    return Err(failure());
                }
                execute(&tx,"UPDATE webdock_auth.hosting_operation o SET status='needs-reconciliation',generation=generation+1,lease_until=NULL,result=jsonb_build_object('error','recovery_policy_revoked') FROM webdock_auth.hosting_recovery_job j WHERE o.id=j.operation_id AND j.policy_id=$1 AND o.status IN('queued','running','needs-reconciliation')",vec![policy_id.clone().into()]).await?;
                json!({"policyID":policy_id,"revision":revision+1,"status":"revoked"})
            }
            RecoveryCommand::Backup {
                policy_id,
                revision,
                ..
            } => {
                contracts::record_id(&policy_id)?;
                let p=one(&tx,"SELECT to_jsonb(p) AS value FROM webdock_auth.hosting_recovery_policy p WHERE id=$1 AND revision=$2 AND revoked_at IS NULL AND webdock_rls.recovery_policy_live(id,cluster_id) FOR UPDATE",vec![policy_id.into(),(revision as i64).into()]).await?.map(object).transpose()?.ok_or_else(failure)?;
                json!({"operationID":self.enqueue_recovery(&tx,&p,RecoveryAction::Backup,None,None).await?,"status":"queued"})
            }
            RecoveryCommand::Restore {
                recovery_set_id,
                target,
                ..
            } => {
                contracts::record_id(&recovery_set_id)?;
                let set=one(&tx,"SELECT to_jsonb(s) AS value FROM webdock_auth.hosting_recovery_set s WHERE id=$1 AND referenced",vec![recovery_set_id.clone().into()]).await?.map(object).transpose()?.ok_or_else(failure)?;
                let p=one(&tx,"SELECT to_jsonb(p) AS value FROM webdock_auth.hosting_recovery_policy p WHERE id=$1 AND revoked_at IS NULL AND webdock_rls.recovery_policy_live(id,cluster_id) FOR UPDATE",vec![field(&set,"policy_id")?.into()]).await?.map(object).transpose()?.ok_or_else(failure)?;
                // The private runtime configuration predeclares exact fresh target profiles.
                // API input may select their ID only; no connection URL, key or recipient.
                if target.as_object().is_none_or(|m| m.len() != 1)
                    || target["profileID"].as_str().is_none()
                {
                    return Err(error(400, "Choose a predeclared isolated recovery target."));
                }
                let capabilities=one(&tx,"SELECT observation#>'{capabilities,nativeRecovery}' AS value FROM webdock_auth.hosting_agent WHERE cluster_id=$1 AND NOT revoked AND last_seen>clock_timestamp()-interval '90 seconds' FOR SHARE",vec![field(&p,"cluster_id")?.into()]).await?.map(native_recovery).transpose()?.ok_or_else(failure)?;
                let declared = capabilities["targets"]
                    .as_array()
                    .and_then(|targets| {
                        targets.iter().find(|t| {
                            t["id"] == target["profileID"] && t["profile"] == p["profile"]
                        })
                    })
                    .cloned()
                    .ok_or_else(failure)?;
                if numeric(&declared, "capacityBytes")? < numeric(&p, "capacity_bytes")? {
                    return Err(error(409, "Recovery target capacity is not reserved."));
                }
                let mut target = declared.clone();
                target["profileID"] = declared["id"].clone();
                if p["profile"] == "ntfy-sqlite-offline-v1" {
                    let runtime: webdock_supervisor::RuntimeSpec =
                        serde_json::from_value(declared["runtime"].clone())
                            .map_err(|_| failure())?;
                    runtime.validate().map_err(|_| failure())?;
                    let initial_app = Self::app(&tx, runtime.resource_id.as_str()).await?;
                    // Match ordinary Hosting allocation serialization: customer plan, then cluster.
                    Self::lock(&tx, format!("plan:{}", field(&initial_app, "customer_id")?))
                        .await?;
                    Self::lock(&tx, format!("hosting-cluster:{}", field(&p, "cluster_id")?))
                        .await?;
                    one(
                        &tx,
                        "SELECT id FROM webdock_auth.hosting_app WHERE id=$1 FOR UPDATE",
                        vec![runtime.resource_id.as_str().into()],
                    )
                    .await?
                    .ok_or_else(failure)?;
                    let app = Self::app(&tx, runtime.resource_id.as_str()).await?;
                    if runtime.project_id.as_str() != field(&app, "project_id")?
                        || app["lifecycle"] != "draft"
                        || !app["observed_uid"].is_null()
                        || app["project_id"] != p["source"]["projectID"]
                        || app["cluster_id"] != p["cluster_id"]
                        || app["spec"]["image"] != runtime.image
                        || app["spec"]["args"] != json!(runtime.arguments)
                        || app["spec"]["volumeBytes"]
                            != runtime
                                .volume
                                .as_ref()
                                .and_then(|v| v.size_bytes)
                                .map_or(Value::Null, |n| json!(n))
                    {
                        return Err(error(409, "A fresh owned application draft is required."));
                    }
                    Self::authorize(
                        &tx,
                        &native_actor,
                        app["project_id"].as_str(),
                        None,
                        true,
                        true,
                        true,
                    )
                    .await?;
                    let mut demand = contracts::app_demand(&app["spec"], false)?;
                    let encrypted = contracts::amount(
                        &set["proof"]["objects"]
                            .as_array()
                            .and_then(|objects| objects.first())
                            .ok_or_else(failure)?["sizeBytes"],
                    )?;
                    let staging = encrypted
                        .checked_mul(2)
                        .and_then(|n| n.checked_add(numeric(&p, "capacity_bytes").ok()? as u64))
                        .and_then(|n| n.checked_add(67108864))
                        .ok_or_else(failure)?;
                    demand["volumeBytes"] = json!(
                        contracts::amount(&demand["volumeBytes"])?
                            .checked_add(staging)
                            .ok_or_else(failure)?
                    );
                    demand["cpuMillicores"] =
                        json!(contracts::amount(&demand["cpuMillicores"])? + 200);
                    demand["memoryBytes"] =
                        json!(contracts::amount(&demand["memoryBytes"])? + 402653184);
                    demand["ephemeralBytes"] =
                        json!(contracts::amount(&demand["ephemeralBytes"])? + 83886080);
                    let subscription =
                        Self::limits(&tx, field(&app, "customer_id")?, None, None).await?;
                    let provider = one(
                        &tx,
                        "SELECT provider FROM webdock_auth.hosting_project WHERE project_id=$1",
                        vec![field(&app, "project_id")?.into()],
                    )
                    .await?
                    .ok_or_else(failure)?
                    .try_get::<String>("", "provider")
                    .map_err(db_error)?;
                    Self::check_demand(
                        &tx,
                        field(&app, "customer_id")?,
                        field(&app, "project_id")?,
                        field(&p, "cluster_id")?,
                        &provider,
                        &demand,
                        numeric(&subscription, "subscriptionRevision")?,
                    )
                    .await?;
                    target["reservationDemand"] = demand;
                    target["applicationSpec"] = app["spec"].clone();
                    target["customerID"] = app["customer_id"].clone();
                }
                json!({"operationID":self.enqueue_recovery(&tx,&p,RecoveryAction::Restore,Some(target),Some(recovery_set_id)).await?,"status":"queued","dispatch":"quarantined"})
            }
            RecoveryCommand::Status { .. } => unreachable!(),
            RecoveryCommand::Export { .. } => unreachable!(),
        };
        self.audit(
            &tx,
            &actor.user_id,
            field(&input, "action")?,
            result["policyID"]
                .as_str()
                .or_else(|| result["operationID"].as_str())
                .unwrap_or("recovery"),
            "accepted",
        )
        .await?;
        self.record_result(
            &tx,
            &native_actor,
            &input,
            (request_hash, &result),
            &Access {
                customer: String::new(),
                project: Value::Null,
                operator: true,
                live: true,
                can_write: true,
            },
            None,
        )
        .await?;
        browser.verify().await?;
        tx.commit().await.map_err(db_error)?;
        Ok(result)
    }

    async fn enqueue_recovery(
        &self,
        tx: &DatabaseTransaction,
        p: &Value,
        action: RecoveryAction,
        target: Option<Value>,
        recovery_set_id: Option<String>,
    ) -> Result<String> {
        if one(tx,"SELECT 1 FROM webdock_auth.hosting_recovery_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id WHERE j.policy_id=$1 AND o.status IN('queued','running','needs-reconciliation')",vec![field(p,"id")?.into()]).await?.is_some(){return Err(error(409,"A recovery operation is already pending reconciliation."));}
        let id = self.id().await?;
        let epoch = one(
            tx,
            "SELECT webdock_rls.current_recovery_epoch()::text AS epoch",
            vec![],
        )
        .await?
        .ok_or_else(failure)?
        .try_get::<String>("", "epoch")
        .map_err(db_error)?;
        let epoch = if p["profile"] == "mail-installation-v1" && action == RecoveryAction::Backup {
            uuid_token()
        } else {
            epoch
        };
        let spec = RecoverySpec {
            version: 1,
            operation_id: id.clone(),
            policy_id: field(p, "id")?.into(),
            policy_revision: numeric(p, "revision")? as u64,
            cluster_id: field(p, "cluster_id")?.into(),
            profile: serde_json::from_value(p["profile"].clone()).map_err(|_| failure())?,
            action,
            source: p["source"].clone(),
            target,
            recovery_set_id,
            epoch,
            capacity_bytes: numeric(p, "capacity_bytes")? as u64,
        };
        let hash = spec.fingerprint().map_err(|_| failure())?;
        let resource_scope = if spec.profile == RecoveryProfile::NtfySqliteOfflineV1 {
            if action == RecoveryAction::Restore {
                spec.target.as_ref().map(|target| {
                    (
                        target["runtime"]["resourceId"].clone(),
                        target["runtime"]["projectId"].clone(),
                        target["customerID"].clone(),
                    )
                })
            } else {
                Some((
                    p["source"]["appID"].clone(),
                    p["source"]["projectID"].clone(),
                    p["source"]["customerID"].clone(),
                ))
            }
        } else {
            None
        };
        let restore_target = spec.target.clone();
        let recovery_spec = spec;
        let spec = serde_json::to_value(&recovery_spec).map_err(|_| failure())?;
        execute(tx,"INSERT INTO webdock_auth.hosting_operation(id,subject,cluster_id,action,idempotency_key,payload_hash,status,desired) VALUES($1,$2,$3,$4,$5,$6,'queued',$7)",vec![id.clone().into(),field(p,"approved_by")?.into(),field(p,"cluster_id")?.into(),format!("recovery.{}",if action==RecoveryAction::Backup{"backup"}else{"restore"}).into(),format!("recovery-native-{id}").into(),hash.clone().into(),spec.clone().into()]).await?;
        execute(tx,"INSERT INTO webdock_auth.hosting_recovery_job(operation_id,policy_id,policy_revision,spec,request_sha256) VALUES($1,$2,$3,$4,$5)",vec![id.clone().into(),field(p,"id")?.into(),numeric(p,"revision")?.into(),spec.into(),hash.into()]).await?;
        self.reserve_recovery_target(tx, p, &recovery_spec, &id)
            .await?;
        if let Some((app, project, customer)) = resource_scope {
            execute(tx,"UPDATE webdock_auth.hosting_operation SET app_id=$2,project_id=$3,customer_id=$4 WHERE id=$1",vec![id.clone().into(),app.as_str().ok_or_else(failure)?.into(),project.as_str().ok_or_else(failure)?.into(),customer.as_str().ok_or_else(failure)?.into()]).await?;
            if action == RecoveryAction::Restore {
                let target = restore_target.as_ref().ok_or_else(failure)?;
                execute(tx,"INSERT INTO webdock_auth.hosting_reservation(operation_id,customer_id,project_id,cluster_id,app_id,demand,status) VALUES($1,$2,$3,$4,$5,$6,'reserved')",vec![id.clone().into(),customer.as_str().ok_or_else(failure)?.into(),project.as_str().ok_or_else(failure)?.into(),field(p,"cluster_id")?.into(),app.as_str().ok_or_else(failure)?.into(),target["reservationDemand"].clone().into()]).await?;
                let transitioned=execute(tx,"UPDATE webdock_auth.hosting_app SET lifecycle='deployed',status='restoring',operation_id=$2 WHERE id=$1 AND lifecycle='draft' AND observed_uid IS NULL",vec![app.as_str().ok_or_else(failure)?.into(),id.clone().into()]).await?;
                if transitioned != 1 {
                    return Err(error(
                        409,
                        "Recovery target intent changed; no reservation was committed.",
                    ));
                }
            }
        }
        execute(tx,"UPDATE webdock_auth.hosting_recovery_policy SET next_run_at=clock_timestamp()+make_interval(secs=>interval_seconds),observed_status='queued',last_error=NULL WHERE id=$1",vec![field(p,"id")?.into()]).await?;
        Ok(id)
    }

    pub async fn claim_recovery(&self, agent: &AgentIdentity) -> Result<Option<RecoveryClaim>> {
        let tx = self.begin_authenticated_agent(agent).await?;
        execute(&tx,"UPDATE webdock_auth.hosting_operation o SET status='needs-reconciliation',result=jsonb_build_object('error','recovery_execution_uncertain') FROM webdock_auth.hosting_recovery_job j WHERE o.id=j.operation_id AND o.cluster_id=$1 AND o.status='running' AND o.lease_until<clock_timestamp()",vec![agent.cluster.clone().into()]).await?;
        let due=one(&tx,"SELECT to_jsonb(p) AS value FROM webdock_auth.hosting_recovery_policy p WHERE cluster_id=$1 AND revoked_at IS NULL AND next_run_at<=clock_timestamp() AND webdock_rls.recovery_policy_live(id,cluster_id) AND NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id WHERE j.policy_id=p.id AND o.status IN('queued','running','needs-reconciliation')) ORDER BY next_run_at,id FOR UPDATE SKIP LOCKED LIMIT 1",vec![agent.cluster.clone().into()]).await?.map(object).transpose()?;
        if let Some(p) = due {
            self.enqueue_recovery(&tx, &p, RecoveryAction::Backup, None, None)
                .await?;
        }
        let job=one(&tx,"SELECT to_jsonb(j) AS value FROM webdock_auth.hosting_recovery_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id JOIN webdock_auth.hosting_recovery_policy p ON p.id=j.policy_id WHERE o.cluster_id=$1 AND o.status='queued' AND p.revision=j.policy_revision AND webdock_rls.recovery_policy_live(p.id,p.cluster_id) ORDER BY o.created_at,o.id FOR UPDATE OF o SKIP LOCKED LIMIT 1",vec![agent.cluster.clone().into()]).await?.map(object).transpose()?;
        let Some(j) = job else {
            tx.commit().await.map_err(db_error)?;
            return Ok(None);
        };
        let lease_token = uuid_token();
        let row=one(&tx,"UPDATE webdock_auth.hosting_operation SET status='running',generation=generation+1,lease_until=clock_timestamp()+interval '300 seconds' WHERE id=$1 RETURNING generation,lease_until",vec![field(&j,"operation_id")?.into()]).await?.ok_or_else(failure)?;
        execute(
            &tx,
            "UPDATE webdock_auth.hosting_recovery_job SET lease_hash=$2,agent_generation=$3 WHERE operation_id=$1",
            vec![
                field(&j, "operation_id")?.into(),
                digest(&lease_token).into(),
                agent.generation.into(),
            ],
        )
        .await?;
        let claim = RecoveryClaim {
            spec: serde_json::from_value(j["spec"].clone()).map_err(|_| failure())?,
            lease: RecoveryLease {
                generation: row.try_get::<i32>("", "generation").map_err(db_error)? as u64,
                token: lease_token,
                expires_at: row.try_get("", "lease_until").map_err(db_error)?,
            },
            checkpoint: (!j["checkpoint"].is_null()).then(|| j["checkpoint"].clone()),
        };
        claim.validate().map_err(|_| failure())?;
        tx.commit().await.map_err(db_error)?;
        Ok(Some(claim))
    }

    async fn recovery_guard_in(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
    ) -> Result<Value> {
        c.validate().map_err(|_| failure())?;
        if c.spec.cluster_id != agent.cluster {
            return Err(failure());
        }
        if app_source::app_backup(c) {
            self.guard_app_source(tx, agent, c).await?;
        }
        let row=one(tx,"SELECT to_jsonb(j)||jsonb_build_object('profile',p.profile,'approved_by',p.approved_by,'destination',p.destination) AS value FROM webdock_auth.hosting_recovery_job j JOIN webdock_auth.hosting_recovery_policy p ON p.id=j.policy_id JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id WHERE o.id=$1 AND o.cluster_id=$2 AND o.status='running' AND o.generation=$3 AND o.lease_until>clock_timestamp() AND j.lease_hash=$4 AND j.agent_generation=$5 AND p.revision=j.policy_revision AND webdock_rls.recovery_policy_live(p.id,p.cluster_id) FOR SHARE OF o,p",vec![c.spec.operation_id.clone().into(),agent.cluster.clone().into(),(c.lease.generation as i32).into(),digest(&c.lease.token).into(),agent.generation.into()]).await?.map(object).transpose()?.ok_or_else(failure)?;
        if row["spec"] != serde_json::to_value(&c.spec).map_err(|_| failure())?
            || row["request_sha256"] != c.spec.fingerprint().map_err(|_| failure())?
        {
            return Err(failure());
        }
        if c.spec.profile == RecoveryProfile::NtfySqliteOfflineV1
            && c.spec.action == RecoveryAction::Restore
        {
            let target = c.spec.target.as_ref().ok_or_else(failure)?;
            let runtime: webdock_supervisor::RuntimeSpec =
                serde_json::from_value(target["runtime"].clone()).map_err(|_| failure())?;
            let app = Self::app(tx, runtime.resource_id.as_str()).await?;
            if app["operation_id"] != c.spec.operation_id
                || app["status"] != "restoring"
                || app["revision"] != runtime.revision
                || app["spec"] != target["applicationSpec"]
                || app["project_id"] != runtime.project_id.as_str()
                || app["cluster_id"] != agent.cluster
            {
                return Err(failure());
            }
        }
        Ok(row)
    }
    pub async fn guard_recovery(&self, agent: &AgentIdentity, c: &RecoveryClaim) -> Result<()> {
        let effect = if app_source::app_backup(c) {
            Some(app_source::app_effect_guard(c)?)
        } else {
            None
        };
        let tx = self.begin_recovery_guarded(agent, c).await?;
        self.recovery_guard_in(&tx, agent, c).await?;
        if let Some(effect) = effect {
            self.guard_app_effect(&tx, agent, c, effect).await?;
        }
        tx.commit().await.map_err(db_error)
    }
    /// Called only after the RPC listener authenticated this exact enrolled wire identity.
    pub async fn bind_recovery_transport(
        &self,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
        installation: &str,
        agent_id: &str,
    ) -> Result<()> {
        let tx = self.begin_recovery_guarded(agent, c).await?;
        self.recovery_guard_in(&tx, agent, c).await?;
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.recovery_bind_transport($1,$2,$3,$4::uuid,$5,$6,$7) AS allowed",
                vec![
                    c.spec.operation_id.clone().into(),
                    (c.lease.generation as i32).into(),
                    digest(&c.lease.token).into(),
                    c.spec.epoch.clone().into(),
                    agent.generation.into(),
                    installation.into(),
                    agent_id.into(),
                ],
            ))
            .await
            .map_err(recovery_bind_db_error)?
            .ok_or_else(failure)?;
        if !row.try_get::<bool>("", "allowed").map_err(db_error)? {
            return Err(failure());
        }
        tx.commit().await.map_err(db_error)
    }
    pub async fn admit_recovery_source_fence(
        &self,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
        participants: Value,
    ) -> Result<Value> {
        let tx = self.begin_recovery_guarded(agent, c).await?;
        self.recovery_guard_in(&tx, agent, c).await?;
        let value = recovery_history_one(
            &tx,
            "SELECT webdock_rls.recovery_admit_source_fence($1,$2,$3,$4::uuid,$5,$6,$7) AS value",
            vec![
                c.spec.operation_id.clone().into(),
                (c.lease.generation as i32).into(),
                digest(&c.lease.token).into(),
                c.spec.epoch.clone().into(),
                participants.into(),
                self.id().await?.into(),
                self.id().await?.into(),
            ],
            RecoveryHistoryBoundary::MailAdmit,
        )
        .await?
        .map(object)
        .transpose()?
        .ok_or_else(failure)?;
        tx.commit().await.map_err(db_error)?;
        Ok(value)
    }
    pub async fn drain_recovery_source_fence(
        &self,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
        proof: Value,
    ) -> Result<()> {
        let tx = self.begin_recovery_guarded(agent, c).await?;
        self.recovery_guard_in(&tx, agent, c).await?;
        let allowed = recovery_history_one(
            &tx,
            "SELECT webdock_rls.recovery_mark_source_drained($1,$2,$3,$4::uuid,$5,$6) AS allowed",
            vec![
                c.spec.operation_id.clone().into(),
                (c.lease.generation as i32).into(),
                digest(&c.lease.token).into(),
                c.spec.epoch.clone().into(),
                proof.into(),
                self.id().await?.into(),
            ],
            RecoveryHistoryBoundary::MailDrain,
        )
        .await?
        .ok_or_else(failure)?
        .try_get::<bool>("", "allowed")
        .map_err(db_error)?;
        if !allowed {
            return Err(failure());
        }
        tx.commit().await.map_err(db_error)
    }
    pub async fn resume_recovery_mail(
        &self,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
    ) -> Result<()> {
        let tx = self.begin_recovery_guarded(agent, c).await?;
        self.recovery_guard_in(&tx, agent, c).await?;
        let allowed = recovery_history_one(
            &tx,
            "SELECT webdock_rls.recovery_source_resume($1,$2,$3,$4::uuid,$5,$6) AS allowed",
            vec![
                c.spec.operation_id.clone().into(),
                (c.lease.generation as i32).into(),
                digest(&c.lease.token).into(),
                c.spec.epoch.clone().into(),
                self.id().await?.into(),
                self.id().await?.into(),
            ],
            RecoveryHistoryBoundary::MailResume,
        )
        .await?
        .ok_or_else(failure)?
        .try_get::<bool>("", "allowed")
        .map_err(db_error)?;
        if !allowed {
            return Err(failure());
        }
        tx.commit().await.map_err(db_error)
    }
    pub async fn recovery_execution_plan(
        &self,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
    ) -> Result<Value> {
        let tx = self.begin_recovery_guarded(agent, c).await?;
        let plan = self.recovery_execution_plan_in(&tx, agent, c).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(plan)
    }
    async fn recovery_execution_plan_in(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
    ) -> Result<Value> {
        let job = self.recovery_guard_in(tx, agent, c).await?;
        let recovery_set = if let Some(id) = &c.spec.recovery_set_id {
            one(tx,"SELECT to_jsonb(s) AS value FROM webdock_auth.hosting_recovery_set s WHERE id=$1 AND policy_id=$2 AND referenced",vec![id.clone().into(),c.spec.policy_id.clone().into()]).await?.map(object).transpose()?.ok_or_else(failure)?
        } else {
            Value::Null
        };
        let inventory = if c.spec.profile == RecoveryProfile::MailInstallationV1 {
            one(
                tx,
                MAIL_INVENTORY_SQL,
                inventory_parameters(
                    &c.spec.operation_id,
                    c.lease.generation,
                    &digest(&c.lease.token),
                    agent.generation,
                ),
            )
            .await?
            .map(object)
            .transpose()?
            .ok_or_else(failure)?
        } else {
            c.spec.source.clone()
        };
        let source_fence = if c.spec.profile == RecoveryProfile::MailInstallationV1 {
            one(
                tx,
                "SELECT webdock_rls.recovery_source_fence_view($1,$2,$3,$4::uuid) AS value",
                vec![
                    c.spec.operation_id.clone().into(),
                    (c.lease.generation as i32).into(),
                    digest(&c.lease.token).into(),
                    c.spec.epoch.clone().into(),
                ],
            )
            .await?
            .map(object)
            .transpose()?
            .ok_or_else(failure)?
        } else {
            Value::Null
        };
        let source_fence = if c.spec.profile == RecoveryProfile::NtfySqliteOfflineV1
            && c.spec.action == RecoveryAction::Backup
        {
            self.app_source_plan(tx, agent, c).await?
        } else {
            source_fence
        };
        let reservation = self.recovery_reservation_plan(tx, agent, c).await?;
        let plan = json!({"version":1,"destination":job["destination"],"source":inventory,"recoverySet":recovery_set,"target":c.spec.target,"sourceFence":source_fence,"reservation":reservation});
        if plan.to_string().len() > 320 * 1024 {
            return Err(error(
                409,
                "Recovery inventory exceeds the qualified frame bound.",
            ));
        }
        Ok(plan)
    }
    pub async fn apply_recovery_checkpoint(
        &self,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
        phase: &str,
        checkpoint: Value,
    ) -> Result<Value> {
        if checkpoint.to_string().len() > 65536
            || !checkpoint.is_object()
            || ![
                "checkpoint",
                "fencing",
                "fenced",
                "resume",
                "source-admit",
                "source-drained",
            ]
            .contains(&phase)
        {
            return Err(error(400, "Invalid recovery checkpoint."));
        }
        let stage = checkpoint["phase"].as_str().unwrap_or("");
        if stage.len() > 64
            || !stage
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err(failure());
        }
        let key = format!("{phase}:{stage}");
        let request_hash = digest(
            &serde_json::to_string(&(
                c.spec.fingerprint().map_err(|_| failure())?,
                c.lease.generation,
                digest(&c.lease.token),
                phase,
                &checkpoint,
            ))
            .map_err(|_| failure())?,
        );
        let app_source = c.spec.profile == RecoveryProfile::NtfySqliteOfflineV1
            && c.spec.action == RecoveryAction::Backup;
        let tx = if app_source {
            self.begin_app_checkpoint(agent).await?
        } else {
            self.begin_authenticated_agent(agent).await?
        };
        if !app_source {
            one(&tx,"SELECT id FROM webdock_auth.hosting_operation WHERE id=$1 AND cluster_id=$2 FOR UPDATE",vec![c.spec.operation_id.clone().into(),agent.cluster.clone().into()]).await?.ok_or_else(failure)?;
        }
        if let Some(prior)=one(&tx,"SELECT request_sha256,response FROM webdock_auth.hosting_recovery_callback WHERE operation_id=$1 AND generation=$2 AND phase_key=$3 AND lease_hash=$4",vec![c.spec.operation_id.clone().into(),(c.lease.generation as i32).into(),key.clone().into(),digest(&c.lease.token).into()]).await? {
            if prior.try_get::<String>("","request_sha256").map_err(db_error)?!=request_hash {return Err(failure());}
            let response=prior.try_get("","response").map_err(db_error)?;
            Self::verify_agent(&tx, agent).await?;
            tx.commit().await.map_err(db_error)?;return Ok(response);
        }
        if app_source {
            self.lock_app_source(&tx, agent, c).await?;
            // Another identical callback may have committed between the initial read and lock.
            if let Some(prior)=one(&tx,"SELECT request_sha256,response FROM webdock_auth.hosting_recovery_callback WHERE operation_id=$1 AND generation=$2 AND phase_key=$3 AND lease_hash=$4",vec![c.spec.operation_id.clone().into(),(c.lease.generation as i32).into(),key.clone().into(),digest(&c.lease.token).into()]).await? {
                if prior.try_get::<String>("","request_sha256").map_err(db_error)?!=request_hash { return Err(failure()); }
                let response=prior.try_get("","response").map_err(db_error)?;
                tx.commit().await.map_err(db_error)?;return Ok(response);
            }
        }
        self.recovery_guard_in(&tx, agent, c).await?;
        if app_source {
            if phase != "checkpoint" {
                return Err(failure());
            }
            self.app_source_checkpoint(&tx, agent, c, &checkpoint)
                .await?;
        }
        let args = vec![
            c.spec.operation_id.clone().into(),
            (c.lease.generation as i32).into(),
            digest(&c.lease.token).into(),
            c.spec.epoch.clone().into(),
        ];
        match phase {
            "source-admit" => {
                let mut args = args;
                args.push(checkpoint["participants"].clone().into());
                args.push(self.id().await?.into());
                args.push(self.id().await?.into());
                recovery_history_one(
                    &tx,
                    "SELECT webdock_rls.recovery_admit_source_fence($1,$2,$3,$4::uuid,$5,$6,$7)",
                    args,
                    RecoveryHistoryBoundary::MailAdmit,
                )
                .await?;
            }
            "source-drained" => {
                let mut args = args;
                args.push(checkpoint.clone().into());
                args.push(self.id().await?.into());
                let allowed=recovery_history_one(&tx,"SELECT webdock_rls.recovery_mark_source_drained($1,$2,$3,$4::uuid,$5,$6) AS allowed",args,RecoveryHistoryBoundary::MailDrain).await?.ok_or_else(failure)?.try_get::<bool>("","allowed").map_err(db_error)?;
                if !allowed {
                    return Err(failure());
                }
            }
            "fencing" | "fenced" => {
                one(
                    &tx,
                    "SELECT webdock_rls.recovery_mail_fence($1,$2,$3,$4)",
                    vec![
                        c.spec.operation_id.clone().into(),
                        (c.lease.generation as i32).into(),
                        digest(&c.lease.token).into(),
                        (phase == "fenced").into(),
                    ],
                )
                .await?;
            }
            "resume" => {
                let mut args = args;
                args.push(self.id().await?.into());
                args.push(self.id().await?.into());
                let allowed = recovery_history_one(
                    &tx,
                    "SELECT webdock_rls.recovery_source_resume($1,$2,$3,$4::uuid,$5,$6) AS allowed",
                    args,
                    RecoveryHistoryBoundary::MailResume,
                )
                .await?
                .ok_or_else(failure)?
                .try_get::<bool>("", "allowed")
                .map_err(db_error)?;
                if !allowed {
                    return Err(failure());
                }
            }
            _ => {}
        }
        execute(
            &tx,
            "UPDATE webdock_auth.hosting_recovery_job SET checkpoint=$2 WHERE operation_id=$1",
            vec![c.spec.operation_id.clone().into(), checkpoint.into()],
        )
        .await?;
        let plan = self.recovery_execution_plan_in(&tx, agent, c).await?;
        execute(&tx,"INSERT INTO webdock_auth.hosting_recovery_callback(operation_id,generation,phase_key,lease_hash,request_sha256,response) VALUES($1,$2,$3,$4,$5,$6)",vec![c.spec.operation_id.clone().into(),(c.lease.generation as i32).into(),key.into(),digest(&c.lease.token).into(),request_hash.into(),plan.clone().into()]).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(plan)
    }
    pub async fn checkpoint_recovery(
        &self,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
        checkpoint: Value,
    ) -> Result<()> {
        if checkpoint.to_string().len() > 65536 || !checkpoint.is_object() {
            return Err(error(400, "Invalid recovery checkpoint."));
        }
        let tx = self.begin_recovery_guarded(agent, c).await?;
        self.recovery_guard_in(&tx, agent, c).await?;
        execute(
            &tx,
            "UPDATE webdock_auth.hosting_recovery_job SET checkpoint=$2 WHERE operation_id=$1",
            vec![c.spec.operation_id.clone().into(), checkpoint.into()],
        )
        .await?;
        tx.commit().await.map_err(db_error)
    }
    pub async fn fence_recovery_mail(
        &self,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
        finish: bool,
    ) -> Result<String> {
        let tx = self.begin_recovery_guarded(agent, c).await?;
        self.recovery_guard_in(&tx, agent, c).await?;
        let epoch = one(
            &tx,
            "SELECT webdock_rls.recovery_mail_fence($1,$2,$3,$4)::text AS epoch",
            vec![
                c.spec.operation_id.clone().into(),
                (c.lease.generation as i32).into(),
                digest(&c.lease.token).into(),
                finish.into(),
            ],
        )
        .await?
        .ok_or_else(failure)?
        .try_get("", "epoch")
        .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(epoch)
    }
    pub async fn complete_recovery(
        &self,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
        completion: &RecoveryCompletion,
    ) -> Result<Value> {
        if completion.generation != c.lease.generation
            || completion.token != c.lease.token
            || !["succeeded", "failed", "needs-reconciliation"].contains(&completion.state.as_str())
            || !completion.proof.is_object()
            || completion.proof.to_string().len() > 65536
            || completion.error_code.as_deref().is_some_and(|code| {
                ![
                    "recovery_authority_or_integrity_denied",
                    "recovery_deadline",
                    "recovery_execution_uncertain",
                ]
                .contains(&code)
            })
        {
            return Err(failure());
        }
        let app_source = app_source::app_backup(c);
        let tx = if app_source {
            self.begin_app_checkpoint(agent).await?
        } else {
            self.begin_authenticated_agent(agent).await?
        };
        // A lost acknowledgement may replay an exact committed receipt after lease
        // expiry. This acknowledges existing state only; it never renews authority.
        let prior_sql = format!(
            "SELECT to_jsonb(j) AS value FROM webdock_auth.hosting_recovery_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id WHERE o.id=$1 AND o.cluster_id=$2 AND o.generation=$3 AND j.agent_generation=$4 AND j.lease_hash=$5 AND j.completed_proof IS NOT NULL AND o.status IN('succeeded','failed','needs-reconciliation') {}",
            if app_source { "" } else { "FOR SHARE OF o" }
        );
        let prior = one(
            &tx,
            &prior_sql,
            vec![
                c.spec.operation_id.clone().into(),
                agent.cluster.clone().into(),
                (c.lease.generation as i32).into(),
                agent.generation.into(),
                digest(&c.lease.token).into(),
            ],
        )
        .await?
        .map(object)
        .transpose()?;
        if let Some(prior) = prior {
            let stored = &prior["completed_proof"];
            if prior["spec"] != serde_json::to_value(&c.spec).map_err(|_| failure())?
                || stored["status"] != completion.state
                || stored["proof"] != completion.proof
                || stored["error"]
                    != completion
                        .error_code
                        .clone()
                        .map_or(Value::Null, Value::String)
            {
                return Err(failure());
            }
            Self::verify_agent(&tx, agent).await?;
            tx.commit().await.map_err(db_error)?;
            return Ok(stored.clone());
        }
        let job = self.recovery_guard_in(&tx, agent, c).await?;
        let mut set_id = None;
        if completion.state == "succeeded" {
            if c.spec.profile == RecoveryProfile::NtfySqliteOfflineV1
                && c.spec.action == RecoveryAction::Backup
            {
                let fence = self.app_source_plan(&tx, agent, c).await?;
                if fence["state"] != "released"
                    || fence["binding"]["fenceFingerprint"]
                        != completion.proof["sourceFenceFingerprint"]
                    || fence["capturedManifestSHA256"] != completion.proof["manifestSha256"]
                {
                    return Err(failure());
                }
            }
            if completion.proof["version"] != 1
                || completion.proof["dispatch"] != "quarantined"
                    && c.spec.action == RecoveryAction::Restore
                || completion.proof["objects"]
                    .as_array()
                    .is_none_or(|v| v.is_empty())
            {
                return Err(failure());
            }
            if c.spec.action == RecoveryAction::Backup {
                let id = self.id().await?;
                execute(&tx,"INSERT INTO webdock_auth.hosting_recovery_set(id,operation_id,policy_id,policy_revision,epoch,proof) VALUES($1,$2,$3,$4,$5::uuid,$6)",vec![id.clone().into(),c.spec.operation_id.clone().into(),c.spec.policy_id.clone().into(),(c.spec.policy_revision as i32).into(),c.spec.epoch.clone().into(),completion.proof.clone().into()]).await?;
                set_id = Some(id);
            }
        }
        let result = json!({"status":completion.state,"recoverySetID":set_id,"proof":completion.proof,"error":completion.error_code});
        if c.spec.profile == RecoveryProfile::NtfySqliteOfflineV1
            && c.spec.action == RecoveryAction::Restore
            && completion.state == "succeeded"
        {
            let target = c.spec.target.as_ref().ok_or_else(failure)?;
            let runtime: webdock_supervisor::RuntimeSpec =
                serde_json::from_value(target["runtime"].clone()).map_err(|_| failure())?;
            let volume = &completion.proof["targetVolume"];
            if completion.proof["networkMode"] != "none"
                || completion.proof["targetNonce"] != target["nonce"]
                || volume["appID"] != runtime.resource_id.as_str()
                || volume["projectID"] != runtime.project_id.as_str()
                || volume["clusterID"] != agent.cluster
                || volume["uuid"] == completion.proof["sourceUUID"]
                || completion.proof["runtimeID"].as_str().is_none()
            {
                return Err(failure());
            }
            execute(&tx,"UPDATE webdock_auth.hosting_app SET status='quarantined',observed_uid=$2,observed_revision=revision,observed_spec=spec,last_error='Restored target is isolated; operator cutover is required.' WHERE id=$1 AND operation_id=$3",vec![runtime.resource_id.as_str().into(),completion.proof["runtimeID"].as_str().ok_or_else(failure)?.into(),c.spec.operation_id.clone().into()]).await?;
            execute(
                &tx,
                "UPDATE webdock_auth.hosting_reservation SET status='active' WHERE operation_id=$1",
                vec![c.spec.operation_id.clone().into()],
            )
            .await?;
        }
        if c.spec.action == RecoveryAction::Restore && completion.state == "succeeded" {
            execute(&tx,"UPDATE webdock_auth.hosting_recovery_reservation SET state='held' WHERE operation_id=$1 AND state='reserved'",vec![c.spec.operation_id.clone().into()]).await?;
        }
        execute(&tx,"UPDATE webdock_auth.hosting_operation SET status=$2,result=$3,lease_until=NULL WHERE id=$1",vec![c.spec.operation_id.clone().into(),completion.state.clone().into(),result.clone().into()]).await?;
        execute(
            &tx,
            "UPDATE webdock_auth.hosting_recovery_job SET completed_proof=$2 WHERE operation_id=$1",
            vec![c.spec.operation_id.clone().into(), result.clone().into()],
        )
        .await?;
        execute(&tx,"UPDATE webdock_auth.hosting_recovery_policy SET observed_status=$2,last_error=$3,last_success_at=CASE WHEN $2='succeeded' THEN clock_timestamp() ELSE last_success_at END WHERE id=$1",vec![c.spec.policy_id.clone().into(),completion.state.clone().into(),completion.error_code.clone().into()]).await?;
        self.audit(
            &tx,
            field(&job, "approved_by")?,
            "recovery.observed",
            &c.spec.operation_id,
            &completion.state,
        )
        .await?;
        tx.commit().await.map_err(db_error)?;
        Ok(result)
    }
}

fn recovery_bind_db_error(cause: sea_orm::DbErr) -> crate::HostingError {
    if std::env::var("WEBDOCK_RECOVERY_COMPONENT_TEST").as_deref() == Ok("1") {
        let (class, state) = recovery_db_observation(&cause);
        eprintln!(
            "recovery_component database_failure boundary=recovery_bind_transport class={class} sqlstate={}",
            state.as_deref().unwrap_or("unavailable")
        );
    }
    db_error(cause)
}
#[derive(Clone, Copy)]
enum RecoveryHistoryBoundary {
    MailAdmit,
    MailDrain,
    MailResume,
    AppAdmit,
    AppDrain,
    AppResume,
}
impl RecoveryHistoryBoundary {
    fn label(self) -> &'static str {
        match self {
            Self::MailAdmit => "mail_admit",
            Self::MailDrain => "mail_drain",
            Self::MailResume => "mail_resume",
            Self::AppAdmit => "app_admit",
            Self::AppDrain => "app_drain",
            Self::AppResume => "app_resume",
        }
    }
}
fn recovery_history_identifier(
    name: Option<&str>,
    allowed: &'static [&'static str],
) -> &'static str {
    match name {
        None => "unavailable",
        Some(name) => allowed
            .iter()
            .copied()
            .find(|known| *known == name)
            .unwrap_or("other"),
    }
}
async fn recovery_history_one(
    db: &impl ConnectionTrait,
    sql: &str,
    values: Vec<SqlValue>,
    boundary: RecoveryHistoryBoundary,
) -> Result<Option<sea_orm::QueryResult>> {
    db.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,sql,values)).await.map_err(|cause| {
        if std::env::var("WEBDOCK_RECOVERY_COMPONENT_TEST").as_deref()==Ok("1") {
            let (class,state)=recovery_db_observation(&cause);
            let pg=match &cause {
                sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(error))|sea_orm::DbErr::Exec(sea_orm::RuntimeErr::SqlxError(error))=>error.as_database_error().and_then(|error|error.try_downcast_ref::<sea_orm::sqlx::postgres::PgDatabaseError>()),
                _=>None,
            };
            let table=recovery_history_identifier(pg.and_then(|e|e.table()),&["hosting_recovery_source_fence","hosting_recovery_source_event","hosting_audit","hosting_recovery_callback","hosting_recovery_job"]);
            let column=recovery_history_identifier(pg.and_then(|e|e.column()),&["id","operation_id","policy_id","policy_revision","operation_generation","agent_generation","epoch","source_kind","source_app_id","source_app_revision","source_observed_uid","source_spec_sha256","source_environment_sha256","source_operation_id","source_operation_generation","source_operation_status","source_operation_fingerprint","plan_sha256","participants","state","created_at","event","proof","subject","action","target_id","outcome","generation","phase_key","lease_hash","request_sha256","response"]);
            eprintln!("recovery_component database_failure boundary={} class={class} sqlstate={} table={table} column={column}",boundary.label(),state.as_deref().unwrap_or("unavailable"));
        }
        db_error(cause)
    })
}
fn uuid_token() -> String {
    let mut b = rand_bytes();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{}-{}-{}-{}-{}",
        hex::encode(&b[..4]),
        hex::encode(&b[4..6]),
        hex::encode(&b[6..8]),
        hex::encode(&b[8..10]),
        hex::encode(&b[10..16])
    )
}

#[cfg(test)]
mod review_sql_tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires the exclusively owned recovery schema fixture and real runtime-role TLS login"]
    async fn inventory_actual_sql_binding_reaches_the_authority_gate_with_four_values() {
        let raw = std::env::var("WEBDOCK_RECOVERY_SQL_TEST_URL").unwrap();
        let url = url::Url::parse(&raw).unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert_eq!(url.path(), "/webdock_recovery_fixture");
        assert_eq!(url.username(), "webdock_worker_runtime");
        let db = sea_orm::Database::connect(raw).await.unwrap();
        let result = db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                MAIL_INVENTORY_SQL,
                inventory_parameters("1", 1, &"a".repeat(64), 1),
            ))
            .await;
        assert!(result.is_err());
        let error = result.err().unwrap().to_string();
        assert!(
            error.contains("Current recovery lease required"),
            "inventory bind must reach actual SQL authority gate instead of protocol/placeholder error"
        );
        db.close().await.unwrap();
    }
}

#[cfg(test)]
mod nullable_native_recovery_tests {
    use super::*;
    #[test]
    fn absent_or_json_null_capability_is_closed_domain_failure() {
        for value in [None, Some(Value::Null)] {
            let error = native_recovery_value(Ok(value)).unwrap_err();
            assert_eq!(error.status, 409);
            assert_eq!(
                error.message,
                "Current recovery policy, resource and enrolled agent are required."
            );
        }
    }
    #[test]
    fn nonnull_values_are_preserved_without_inventing_capabilities() {
        for value in [json!({}), json!({"version":2,"profiles":[],"targets":[]})] {
            assert_eq!(
                native_recovery_value(Ok(Some(value.clone()))).unwrap(),
                value
            );
        }
    }
    #[test]
    fn actual_database_and_nonnull_decode_errors_stay_unavailable() {
        for error in [
            sea_orm::DbErr::Type("wrong nonnull SQL type".into()),
            sea_orm::DbErr::Custom("query failure".into()),
        ] {
            assert_eq!(native_recovery_value(Err(error)).unwrap_err().status, 503);
        }
    }
}

#[cfg(test)]
mod transport_binding_source_tests {
    use super::*;
    #[test]
    fn mail_reserved_epoch_and_compensation_binding_do_not_rewrite_mail_fences() {
        let sql = include_str!("../../persistence/migrations/0054_source_recovery_fences.sql");
        let function = sql
            .split("CREATE FUNCTION webdock_rls.recovery_bind_transport(")
            .nth(1)
            .unwrap()
            .split("END $$;")
            .next()
            .unwrap();
        assert!(function.contains("initial_binding AND epoch_row.epoch<>expected_epoch"));
        assert!(function.contains("epoch_row.state='quarantined'"));
        assert!(function.contains("epoch_row.source_resume_allowed=false"));
        assert!(function.contains("j.checkpoint->>'phase'='same-fence-compensation'"));
        assert!(function.contains("f.state='resumed' AND f.epoch=expected_epoch"));
        assert!(function.contains("WHEN 'fencing' THEN epoch_row.epoch=expected_epoch AND epoch_row.operation_id=requested_operation AND epoch_row.generation=requested_generation AND epoch_row.intent_revision=p.revision"));
        assert!(function.contains("WHEN 'fenced' THEN epoch_row.epoch=expected_epoch AND epoch_row.operation_id=requested_operation AND epoch_row.generation=requested_generation AND epoch_row.intent_revision=p.revision"));
        assert!(function.contains("IF initial_binding IS NOT TRUE THEN RETURN false;END IF;"));
        for (name, expected) in [
            (
                "recovery_mail_fence",
                "dd82e0dd85c898ceea16510a456ab6c3d7b303678dfe139e7d1d80ebfcec3070",
            ),
            (
                "recovery_source_resume",
                "6b374b7f4052c9972c3dd586190d18697a5b8445c1ee4b5f7aff8fcd16954cf7",
            ),
        ] {
            let declaration = if name == "recovery_source_resume" {
                "CREATE FUNCTION"
            } else {
                "CREATE OR REPLACE FUNCTION"
            };
            let body = sql
                .split(&format!("{declaration} webdock_rls.{name}("))
                .nth(1)
                .unwrap()
                .split("END $$;")
                .next()
                .unwrap();
            // Only the approved native-ID signature/validation/insert plumbing differs.
            let mut body = body.replace(",event_id text,audit_id text", "");
            if let Some(start) = body.find(" -- Core-native history IDs, never checkpoint input.\n")
            {
                let end = body.find(" -- End native history IDs.\n").unwrap()
                    + " -- End native history IDs.\n".len();
                body.replace_range(start..end, "");
            }
            body = body
                .replace(
                    "source_event(id,operation_id,event,proof) VALUES(event_id,operation,",
                    "source_event(operation_id,event,proof) VALUES(operation,",
                )
                .replace(
                    "VALUES(audit_id,p.approved_by,",
                    "VALUES(webdock_auth.next_snowflake(),p.approved_by,",
                );
            assert_eq!(
                hex::encode(Sha256::digest(body.as_bytes())),
                expected,
                "existing Mail fence protocol changed"
            );
        }
    }
    #[test]
    fn binding_is_worker_only_owner_action_without_transport_table_grants() {
        let sql = include_str!("../../persistence/migrations/0054_source_recovery_fences.sql");
        let function = sql
            .split("CREATE FUNCTION webdock_rls.recovery_bind_transport(")
            .nth(1)
            .expect("narrow binding action missing")
            .split("END $$;")
            .next()
            .unwrap();
        assert!(function.contains("RETURNS boolean"));
        assert!(function.contains("SECURITY DEFINER SET search_path=pg_catalog"));
        assert!(
            function.find("recovery_app_source_lock(").unwrap()
                < function.find("FOR SHARE").unwrap(),
            "app epoch-first guard must precede late row locks"
        );
        assert!(function.contains("e.credential_hash=a.credential_hash"));
        assert!(function.contains("cluster.runtime_installation=requested_installation"));
        assert!(function.contains("agent_identity IS NULL"));
        assert!(!function.contains("RETURN identity"));
        assert!(!sql.contains("GRANT SELECT ON webdock_auth.agent_transport_enrollment"));
        assert!(sql.contains("GRANT EXECUTE ON FUNCTION webdock_rls.recovery_bind_transport(text,integer,text,uuid,integer,text,text) TO webdock_worker_runtime;"));
        let source = include_str!("recovery.rs");
        let body = source
            .split("pub async fn bind_recovery_transport(")
            .nth(1)
            .unwrap()
            .split("pub async fn admit_recovery_source_fence")
            .next()
            .unwrap();
        assert!(!body.contains("FROM webdock_auth.agent_transport_enrollment"));
        assert!(body.contains("SELECT webdock_rls.recovery_bind_transport"));
    }
}

#[cfg(test)]
mod native_source_history_tests {
    use super::*;
    #[test]
    fn diagnostic_identifiers_are_closed_and_never_return_arbitrary_text() {
        assert_eq!(recovery_history_identifier(Some("id"), &["id"]), "id");
        assert_eq!(recovery_history_identifier(None, &["id"]), "unavailable");
        for value in [
            "password=private",
            "https://private.invalid",
            "id\nsecret",
            "source_custom_field",
        ] {
            assert_eq!(recovery_history_identifier(Some(value), &["id"]), "other");
        }
    }
    #[test]
    fn all_source_history_writers_require_explicit_native_ids_and_retire_old_signatures() {
        let sql = include_str!("../../persistence/migrations/0054_source_recovery_fences.sql");
        assert!(
            !sql.contains("webdock_auth.next_snowflake()"),
            "native source history still invokes the unowned legacy allocator"
        );
        for line in sql
            .lines()
            .filter(|line| line.contains("INSERT INTO webdock_auth.hosting_recovery_source_event("))
        {
            assert!(line.contains("source_event(id,operation_id,event,proof)"));
        }
        for signature in [
            "recovery_admit_source_fence(text,integer,text,uuid,jsonb)",
            "recovery_mark_source_drained(text,integer,text,uuid,jsonb)",
            "recovery_source_resume(text,integer,text,uuid)",
            "recovery_app_source_admit(text,integer,text,uuid,integer,jsonb)",
            "recovery_app_source_drained(text,integer,text,uuid,integer,jsonb)",
            "recovery_app_source_resume(text,integer,text,uuid,integer,jsonb)",
        ] {
            assert!(
                sql.contains(&format!("DROP FUNCTION IF EXISTS webdock_rls.{signature};")),
                "obsolete writer entrypoint retained: {signature}"
            );
        }
        let core = include_str!("recovery.rs");
        let app = include_str!("recovery_app.rs");
        for (source, entries) in [
            (
                core,
                vec![
                    ("recovery_admit_source_fence", 7, 2),
                    ("recovery_mark_source_drained", 6, 2),
                    ("recovery_source_resume", 6, 2),
                ],
            ),
            (
                app,
                vec![
                    ("recovery_app_source_admit", 8, 1),
                    ("recovery_app_source_drained", 7, 1),
                    ("recovery_app_source_resume", 8, 1),
                ],
            ),
        ] {
            for (name, arity, count) in entries {
                let needle = format!("SELECT webdock_rls.{name}(");
                let lines = source
                    .lines()
                    .filter(|line| line.contains(&needle))
                    .collect::<Vec<_>>();
                assert_eq!(
                    lines.len(),
                    count,
                    "writer caller inventory changed for {name}"
                );
                let arguments = (1..=arity)
                    .map(|i| {
                        if i == 4 {
                            "$4::uuid".into()
                        } else {
                            format!("${i}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                assert!(
                    lines
                        .iter()
                        .all(|line| line.contains(&format!("{needle}{arguments})"))),
                    "native IDs missing from {name} caller"
                );
            }
        }
        assert!(app.contains("self.id().await?"));
        assert!(!app.contains("checkpoint[\"eventID\"]"));
        assert!(!app.contains("checkpoint[\"auditID\"]"));
    }
}
