use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use subtle::ConstantTimeEq;
use webdock_supervisor::{Action, Command, ObservedState, Phase, RuntimeSpec};
impl HostingService {
    /// Select and derive a native command atomically. Unsupported rows never acquire a lease.
    /// Retained Kubernetes commands continue through the separate legacy execution path.
    pub async fn claim_runtime(&self, agent: &AgentIdentity) -> Result<Option<Command>> {
        let Some(tx) = self.dispatch_tx(agent).await? else {
            return Ok(None);
        };
        let kind = one(
            &tx,
            "SELECT runtime_kind FROM webdock_auth.hosting_cluster WHERE id=$1",
            vec![agent.cluster.clone().into()],
        )
        .await?
        .ok_or_else(|| error(404, "Cluster unavailable."))?
        .try_get::<String>("", "runtime_kind")
        .map_err(db_error)?;
        if !["docker", "kubernetes"].contains(&kind.as_str()) {
            return Err(error(503, "Runtime kind unavailable."));
        }
        let claimed = self.claim_filtered(&tx, agent, true).await?;
        if claimed.is_null() {
            tx.commit().await.map_err(db_error)?;
            return Ok(None);
        }
        let mut op = Self::operation(&tx, agent, field(&claimed, "id")?, false).await?;
        let browser_guard = self.operation_browser_guard(&op).await?;
        op["desired"]["quota"] = claimed["desired"]["quota"].clone();
        op["desired"]["finalQuota"] = claimed["desired"]["finalQuota"].clone();
        execute(
            &tx,
            "UPDATE webdock_auth.hosting_operation SET desired=$2 WHERE id=$1",
            vec![field(&op, "id")?.into(), op["desired"].clone().into()],
        )
        .await?;
        let command = self.native_command(&tx, agent, &op).await?;
        Self::verify_agent(&tx, agent).await?;
        if !self
            .accepted_authority_with_guard(&tx, &op, browser_guard.as_ref())
            .await?
        {
            return Err(error(403, "Operation authority changed."));
        }
        browser_authority::verify(&browser_guard).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(Some(command))
    }
    pub async fn runtime_command(
        &self,
        agent: &AgentIdentity,
        id: &str,
        generation: u64,
    ) -> Result<Command> {
        let tx = self.agent_tx(agent).await?;
        let op = Self::operation(&tx, agent, id, false).await?;
        Self::ensure_app_effect_allowed(&tx, field(&op, "app_id")?, None).await?;
        let browser_guard = self.operation_browser_guard(&op).await?;
        Self::lease(&op, generation as i64)?;
        if !self
            .accepted_authority_with_guard(&tx, &op, browser_guard.as_ref())
            .await?
        {
            return Err(error(403, "Operation authority changed."));
        }
        let command = self.native_command(&tx, agent, &op).await?;
        browser_authority::verify(&browser_guard).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(command)
    }
    pub(super) async fn native_command(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        op: &Value,
    ) -> Result<Command> {
        let cluster = rows(
            tx,
            "SELECT to_jsonb(c) AS value FROM webdock_auth.hosting_cluster c WHERE id=$1",
            vec![agent.cluster.clone().into()],
        )
        .await?
        .pop()
        .ok_or_else(|| error(404, "Cluster unavailable."))?;
        let legacy = cluster["runtime_kind"] == "kubernetes";
        if !legacy && cluster["runtime_kind"] != "docker" {
            return Err(error(503, "Runtime kind unavailable."));
        }
        let desired = &op["desired"];
        if desired.get("autoscalingVersion").is_some() {
            let state = Self::verify_agent(tx, agent).await?;
            contracts::admit_autoscaling(
                true,
                cluster["runtime_kind"].as_str().unwrap_or(""),
                &state["observation"]["capabilities"]["autoscalingVersion"],
            )?;
        }
        if !matches!(
            desired["action"].as_str(),
            Some("apply" | "delete" | "purge-storage")
        ) || desired.get("serviceBindingsVersion").is_some()
        {
            return Err(error(
                503,
                "Native runtime does not support this retained command capability.",
            ));
        }
        if desired["action"] == "apply"
            && op["action"] != "apps.stop"
            && desired["environmentVersion"] == 1
        {
            let state = Self::verify_agent(tx, agent).await?;
            if state["observation"]["capabilities"]["environment"] != true {
                return Err(error(503, "Native environment capability unavailable."));
            }
        }
        Self::verify_public_route(tx, op).await?;
        if desired.get("publicRoute").is_some()
            && (!legacy
                || Self::verify_agent(tx, agent).await?["observation"]["capabilities"]["publicRoutingVersion"]
                    != 1)
        {
            return Err(error(
                503,
                "Qualified public routing capability unavailable.",
            ));
        }
        let spec = &desired["spec"];
        let volume_bytes = spec
            .get("volumeBytes")
            .map(amount)
            .transpose()?
            .unwrap_or(0);
        let runtime = RuntimeSpec {
            autoscaling: spec
                .get("autoscaling")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| error(503, "Invalid persisted scaling policy."))?,
            version: 1,
            installation: field(&cluster, "runtime_installation")?.into(),
            project_id: webdock_core::id::RecordId::parse(field(op, "project_id")?)
                .map_err(|_| error(400, "Invalid project ID."))?,
            resource_id: webdock_core::id::RecordId::parse(field(op, "app_id")?)
                .map_err(|_| error(400, "Invalid application ID."))?,
            revision: numeric(op, "target_revision")? as u64,
            image: field(spec, "image")?.into(),
            arguments: serde_json::from_value(spec["args"].clone())
                .map_err(|_| error(400, "Invalid application arguments."))?,
            cpu_millicores: amount(&spec["cpuMillicores"])? as u32,
            memory_bytes: amount(&spec["memoryBytes"])?,
            ephemeral_bytes: amount(&spec["ephemeralBytes"])?,
            replicas: amount(&spec["replicas"])? as u16,
            port: amount(&spec["port"])? as u16,
            health_path: Some(field(spec, "healthPath")?.into()),
            health_host: spec["healthHost"].as_str().map(str::to_owned),
            volume: (volume_bytes > 0).then_some(webdock_supervisor::RetainedVolume {
                mount_path: "/data".into(),
                size_bytes: Some(volume_bytes),
            }),
            hosting_binding: if legacy {
                Some(webdock_supervisor::HostingBinding {
                    public_route: desired
                        .get("publicRoute")
                        .map(|route| {
                            serde_json::from_value(route.clone())
                                .map_err(|_| error(503, "Invalid persisted public route."))
                        })
                        .transpose()?,
                    cluster_id: webdock_core::id::RecordId::parse(&agent.cluster)
                        .map_err(|_| error(400, "Invalid cluster."))?,
                    quota: serde_json::from_value(desired["quota"].clone())
                        .map_err(|_| error(503, "Persisted namespace quota unavailable."))?,
                    final_quota: serde_json::from_value(desired["finalQuota"].clone())
                        .map_err(|_| error(503, "Persisted final quota unavailable."))?,
                })
            } else {
                None
            },
            hosted_access: if desired["environmentVersion"] == 1 {
                Some(webdock_supervisor::HostedAccess {
                    service_bindings: None,
                    environment: Some(webdock_supervisor::EnvironmentReference {
                        revision: numeric(op, "target_revision")? as u64,
                        source_sha256: hex::encode(Sha256::digest(
                            field(desired, "environmentEncrypted")?.as_bytes(),
                        )),
                    }),
                })
            } else {
                None
            },
            plugin: None,
            mounts: vec![],
        };
        let mut command = Command {
            material: None,
            expected_runtime_id: desired["expectedUID"].as_str().map(str::to_owned),
            operation_id: field(op, "id")?.into(),
            generation: numeric(op, "generation")? as u64,
            lease_until: epoch(&op["lease_until"])
                .ok_or_else(|| error(409, "Lease unavailable."))? as u64,
            authorization: String::new(),
            action: if desired["action"] == "delete" {
                Action::Retire
            } else if desired["action"] == "purge-storage" {
                Action::PurgeStorage
            } else if op["action"] == "apps.stop" {
                Action::Stop
            } else {
                Action::Apply
            },
            spec: runtime,
        };
        command.authorization = self.runtime_mac(agent, &command)?;
        command.validate().map_err(|_| {
            error(
                400,
                "Runtime contract does not support application specification.",
            )
        })?;
        Ok(command)
    }
    pub(super) fn runtime_mac(&self, agent: &AgentIdentity, command: &Command) -> Result<String> {
        let digest = command
            .fingerprint()
            .map_err(|_| error(400, "Invalid runtime command."))?;
        let raw = serde_json::to_vec(&json!([
            "hosting-runtime-v1",
            agent.cluster,
            agent.generation,
            agent.credential_hash,
            command.operation_id,
            command.generation,
            command.lease_until,
            digest
        ]))
        .unwrap();
        let mut mac = Hmac::<Sha256>::new_from_slice(self.rls_secret.as_bytes())
            .map_err(|_| error(503, "Runtime authorization unavailable."))?;
        mac.update(&raw);
        Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
    }
    pub async fn guard_runtime_command(
        &self,
        agent: &AgentIdentity,
        command: &Command,
    ) -> Result<()> {
        let expected = self
            .runtime_command(agent, &command.operation_id, command.generation)
            .await?;
        if expected.lease_until != command.lease_until
            || expected
                .fingerprint()
                .map_err(|_| error(400, "Invalid runtime command."))?
                != command
                    .fingerprint()
                    .map_err(|_| error(400, "Invalid runtime command."))?
            || !bool::from(
                expected
                    .authorization
                    .as_bytes()
                    .ct_eq(command.authorization.as_bytes()),
            )
        {
            return Err(error(403, "Runtime command authority changed."));
        }
        Ok(())
    }
    /// Idempotent failure receipt for native transports, including lost acknowledgments.
    pub async fn complete_runtime_failure(
        &self,
        agent: &AgentIdentity,
        command: &Command,
        error_code: &str,
    ) -> Result<Value> {
        self.complete_runtime_failure_inner(agent, command, error_code, None)
            .await
    }
    pub async fn complete_runtime_failure_with_receipt(
        &self,
        agent: &AgentIdentity,
        command: &Command,
        error_code: &str,
        receipt_sha256: &str,
    ) -> Result<Value> {
        self.complete_runtime_failure_inner(agent, command, error_code, Some(receipt_sha256))
            .await
    }
    async fn complete_runtime_failure_inner(
        &self,
        agent: &AgentIdentity,
        command: &Command,
        error_code: &str,
        receipt_sha256: Option<&str>,
    ) -> Result<Value> {
        let code = if error_code.is_empty() {
            "execution_failed"
        } else {
            error_code
        };
        if code.len() > 64
            || !code
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        {
            return Err(error(400, "Invalid runtime failure code."));
        }
        let tx = self.agent_tx(agent).await?;
        let op = Self::operation(&tx, agent, &command.operation_id, true).await?;
        let expected = self.native_command(&tx, agent, &op).await?;
        let fingerprint = command
            .fingerprint()
            .map_err(|_| error(400, "Invalid runtime command."))?;
        if expected.generation != command.generation
            || expected.lease_until != command.lease_until
            || expected
                .fingerprint()
                .map_err(|_| error(400, "Invalid runtime command."))?
                != fingerprint
            || !bool::from(
                expected
                    .authorization
                    .as_bytes()
                    .ct_eq(command.authorization.as_bytes()),
            )
        {
            return Err(error(403, "Runtime command authority changed."));
        }
        let receipt = json!({"requestSha256":fingerprint,"errorCode":code});
        if ["needs-reconciliation", "failed"].contains(&op["status"].as_str().unwrap_or("")) {
            Self::ensure_app_effect_allowed(&tx, field(&op, "app_id")?, Some(&op)).await?;
            if op["result"]["runtimeReceipt"] != receipt {
                return Err(error(409, "Runtime failure receipt changed."));
            }
            let result = json!({"id":command.operation_id,"status":op["status"]});
            self.record_accepted_delivery(
                &tx,
                agent,
                DeliveryCommand::Mutation(command),
                receipt_sha256,
            )
            .await?;
            tx.commit().await.map_err(db_error)?;
            return Ok(result);
        }
        Self::ensure_app_effect_allowed(&tx, field(&op, "app_id")?, None).await?;
        let browser_guard = self.operation_browser_guard(&op).await?;
        Self::lease(&op, command.generation as i64)?;
        let result=self.complete_with_runtime(&tx,agent,&json!({"id":command.operation_id,"generation":command.generation,"outcome":"failed","error":code}),true,browser_guard.as_ref()).await?;
        execute(&tx,"UPDATE webdock_auth.hosting_operation SET result=jsonb_set(result,'{runtimeReceipt}',$2) WHERE id=$1",vec![command.operation_id.clone().into(),receipt.into()]).await?;
        Self::verify_agent(&tx, agent).await?;
        browser_authority::verify(&browser_guard).await?;
        self.record_accepted_delivery(
            &tx,
            agent,
            DeliveryCommand::Mutation(command),
            receipt_sha256,
        )
        .await?;
        self.admit_completion_commit(&tx, &op, Some(command.lease_until), browser_guard.as_ref())
            .await?;
        tx.commit().await.map_err(db_error)?;
        Ok(result)
    }
    fn runtime_success_receipt(
        &self,
        command: &Command,
        observed: &ObservedState,
    ) -> Result<Value> {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.rls_secret.as_bytes())
            .map_err(|_| error(503, "Runtime receipt unavailable."))?;
        mac.update(b"hosting-runtime-result-v1\0");
        mac.update(
            &serde_json::to_vec(observed)
                .map_err(|_| error(400, "Invalid runtime observation."))?,
        );
        Ok(
            json!({"requestSha256":command.fingerprint().map_err(|_|error(400,"Invalid runtime command."))?,"resultMac":hex::encode(mac.finalize().into_bytes()),"outcome":"succeeded"}),
        )
    }
    pub async fn complete_runtime(
        &self,
        agent: &AgentIdentity,
        command: &Command,
        observed: &ObservedState,
    ) -> Result<Value> {
        self.complete_runtime_inner(agent, command, observed, None)
            .await
    }
    pub async fn complete_runtime_with_receipt(
        &self,
        agent: &AgentIdentity,
        command: &Command,
        observed: &ObservedState,
        receipt_sha256: &str,
    ) -> Result<Value> {
        self.complete_runtime_inner(agent, command, observed, Some(receipt_sha256))
            .await
    }
    async fn complete_runtime_inner(
        &self,
        agent: &AgentIdentity,
        command: &Command,
        observed: &ObservedState,
        receipt_sha256: Option<&str>,
    ) -> Result<Value> {
        let receipt = self.runtime_success_receipt(command, observed)?;
        let tx = self.agent_tx(agent).await?;
        let op = Self::operation(&tx, agent, &command.operation_id, true).await?;
        let expected = self.native_command(&tx, agent, &op).await?;
        if expected.generation != command.generation
            || expected.lease_until != command.lease_until
            || expected
                .fingerprint()
                .map_err(|_| error(400, "Invalid runtime command."))?
                != command
                    .fingerprint()
                    .map_err(|_| error(400, "Invalid runtime command."))?
            || !bool::from(
                expected
                    .authorization
                    .as_bytes()
                    .ct_eq(command.authorization.as_bytes()),
            )
        {
            return Err(error(403, "Runtime command authority changed."));
        }
        if op["status"] == "succeeded" {
            Self::ensure_app_effect_allowed(&tx, field(&op, "app_id")?, Some(&op)).await?;
            if op["result"]["nativeRuntimeReceipt"] != receipt {
                return Err(error(
                    409,
                    "Runtime success receipt changed or is unavailable.",
                ));
            }
            self.record_accepted_delivery(
                &tx,
                agent,
                DeliveryCommand::Mutation(command),
                receipt_sha256,
            )
            .await?;
            tx.commit().await.map_err(db_error)?;
            return Ok(
                json!({"id":command.operation_id,"status":"succeeded","appID":command.spec.resource_id.to_string()}),
            );
        }
        Self::ensure_app_effect_allowed(&tx, field(&op, "app_id")?, None).await?;
        let browser_guard = self.operation_browser_guard(&op).await?;
        Self::lease(&op, command.generation as i64)?;
        if !self
            .accepted_authority_with_guard(&tx, &op, browser_guard.as_ref())
            .await?
        {
            return Err(error(403, "Operation authority changed."));
        }
        if observed.observed_at < command.lease_until.saturating_sub(300)
            || observed.observed_at > now() as u64 + 30
        {
            return Err(error(409, "Runtime observation is stale."));
        }
        if observed.resource_id != command.spec.resource_id.to_string()
            || observed.revision != command.spec.revision
        {
            return Err(error(409, "Runtime observation does not match command."));
        }
        let deleting = matches!(command.action, Action::Retire | Action::PurgeStorage);
        let stopped = command.spec.replicas == 0;
        let converged = if deleting {
            observed.phase == Phase::Absent && observed.ready && observed.runtime_id.is_none()
        } else if stopped {
            observed.phase == Phase::Stopped
        } else {
            observed.phase == Phase::Running && observed.ready
        };
        if !converged {
            return Err(error(
                409,
                "Application has not converged to requested state.",
            ));
        }
        let public_route = command
            .spec
            .hosting_binding
            .as_ref()
            .and_then(|b| b.public_route.as_ref());
        if public_route.is_some()
            && !deleting
            && !stopped
            && !observed
                .conditions
                .iter()
                .any(|c| c.kind == "PublicHttpsReady" && c.satisfied)
        {
            return Err(error(409, "Public address HTTPS has not converged."));
        }
        if let Some(policy) = command.spec.autoscaling.as_ref()
            && !deleting
            && !stopped
        {
            let state = observed
                .autoscaling
                .as_ref()
                .ok_or_else(|| error(409, "Automatic scaling observation required."))?;
            if state.min_replicas != policy.min_replicas
                || state.max_replicas != policy.max_replicas
                || !(policy.min_replicas..=policy.max_replicas).contains(&state.current_replicas)
                || state.ready_replicas != state.current_replicas
                || state.desired_replicas > policy.max_replicas
                || state.reason.len() > 80
            {
                return Err(error(409, "Automatic scaling has not converged."));
            }
        }
        let mut proof = json!({"appID":command.spec.resource_id.to_string(),"revision":command.spec.revision,"namespace":format!("wd-{}",command.spec.project_id),"uid":observed.runtime_id,"ready":true,"deleted":deleting,"replicas":if deleting{0}else{command.spec.replicas}});
        if let Some(state) = &observed.autoscaling {
            proof["replicas"] = json!(state.current_replicas);
            proof["autoscaling"] = json!(state);
        }
        if let Some(volume) = &command.spec.volume {
            let expected = if command.action == Action::PurgeStorage {
                0
            } else {
                volume
                    .size_bytes
                    .ok_or_else(|| error(409, "Verified volume quota required."))?
            };
            let storage = observed
                .storage
                .as_ref()
                .ok_or_else(|| error(409, "Storage proof required."))?;
            if storage.volume_bytes != Some(expected)
                || storage.retained != (command.action == Action::Retire)
            {
                return Err(error(409, "Storage proof does not match operation."));
            }
            proof["storage"] = json!({"volumeBytes":expected,"retained":storage.retained});
        } else if observed
            .storage
            .as_ref()
            .is_some_and(|s| s.retained || s.volume_bytes.is_some_and(|n| n > 0))
        {
            return Err(error(409, "Unexpected retained storage."));
        }
        let result=self.complete_with_runtime(&tx,agent,&json!({"id":command.operation_id,"generation":command.generation,"outcome":"succeeded","proof":proof}),true,browser_guard.as_ref()).await?;
        execute(&tx,"UPDATE webdock_auth.hosting_operation SET result=coalesce(result,'{}')||jsonb_build_object('nativeRuntimeReceipt',$2::jsonb) WHERE id=$1",vec![command.operation_id.clone().into(),receipt.into()]).await?;
        if let Some(route) = public_route {
            execute(&tx,"UPDATE webdock_auth.platform_hostname SET state=$2 WHERE id=$1 AND state<>'retired'",vec![route.allocation_id.to_string().into(),if deleting {"retired"} else if stopped {"allocated"} else {"ready"}.into()]).await?;
        }
        Self::verify_agent(&tx, agent).await?;
        browser_authority::verify(&browser_guard).await?;
        self.record_accepted_delivery(
            &tx,
            agent,
            DeliveryCommand::Mutation(command),
            receipt_sha256,
        )
        .await?;
        self.admit_completion_commit(&tx, &op, Some(command.lease_until), browser_guard.as_ref())
            .await?;
        tx.commit().await.map_err(db_error)?;
        Ok(result)
    }
}
impl HostingService {
    /// Trusted Core transport only. Derives ephemeral material from this operation's immutable
    /// ciphertext after the same live guard; never exposes current mutable app values by ID.
    pub async fn runtime_material(
        &self,
        agent: &AgentIdentity,
        command: &Command,
    ) -> Result<Option<webdock_supervisor::RuntimeMaterial>> {
        let tx = self.agent_tx(agent).await?;
        let op = Self::operation(&tx, agent, &command.operation_id, false).await?;
        let browser_guard = self.operation_browser_guard(&op).await?;
        Self::lease(&op, command.generation as i64)?;
        if !self
            .accepted_authority_with_guard(&tx, &op, browser_guard.as_ref())
            .await?
        {
            return Err(error(403, "Operation authority changed."));
        }
        let expected = self.native_command(&tx, agent, &op).await?;
        if expected.generation != command.generation
            || expected.lease_until != command.lease_until
            || expected
                .fingerprint()
                .map_err(|_| error(400, "Invalid command."))?
                != command
                    .fingerprint()
                    .map_err(|_| error(400, "Invalid command."))?
            || !bool::from(
                expected
                    .authorization
                    .as_bytes()
                    .ct_eq(command.authorization.as_bytes()),
            )
        {
            return Err(error(403, "Runtime command authority changed."));
        }
        let material = if command.action != Action::Apply || command.spec.hosted_access.is_none() {
            None
        } else {
            let sealed = field(&op["desired"], "environmentEncrypted")?;
            let environment = webdock_supervisor::EnvironmentMaterial {
                source_sha256: hex::encode(Sha256::digest(sealed.as_bytes())),
                values: self.environment.open(field(&op, "app_id")?, Some(sealed))?,
            };
            environment
                .validate()
                .map_err(|_| error(503, "Stored environment exceeds runtime contract."))?;
            Some(webdock_supervisor::RuntimeMaterial {
                service_bindings: None,
                operation_id: command.operation_id.clone(),
                request_sha256: command
                    .fingerprint()
                    .map_err(|_| error(400, "Invalid command."))?,
                environment: Some(environment),
            })
        };
        Self::verify_agent(&tx, agent).await?;
        if !self
            .accepted_authority_with_guard(&tx, &op, browser_guard.as_ref())
            .await?
        {
            return Err(error(403, "Operation authority changed."));
        }
        browser_authority::verify(&browser_guard).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(material)
    }
}
