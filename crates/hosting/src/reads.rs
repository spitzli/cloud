//! Durable native reads use the same retained operation and session authority as mutations.
use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use subtle::ConstantTimeEq;
use webdock_supervisor::{
    HostingBinding, ObservedState, Phase, ReadCommand, ReadKind, ResourceMetrics, RetainedVolume,
    RuntimeSpec,
};
impl HostingService {
    pub(super) fn read_mac(&self, agent: &AgentIdentity, command: &ReadCommand) -> Result<String> {
        let fingerprint = command
            .fingerprint()
            .map_err(|_| error(400, "Invalid read command."))?;
        let raw = serde_json::to_vec(&json!([
            "hosting-read-v1",
            agent.cluster,
            agent.generation,
            agent.credential_hash,
            command.operation_id,
            command.generation,
            command.lease_until,
            fingerprint
        ]))
        .unwrap();
        let mut mac = Hmac::<Sha256>::new_from_slice(self.rls_secret.as_bytes())
            .map_err(|_| error(503, "Read signing unavailable."))?;
        mac.update(&raw);
        Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
    }
    pub async fn claim_read(&self, agent: &AgentIdentity) -> Result<Option<ReadCommand>> {
        let Some(tx) = self.dispatch_tx(agent).await? else {
            return Ok(None);
        };
        let claim = self.claim_filtered_mode(&tx, agent, 2).await?;
        if claim.is_null() {
            tx.commit().await.map_err(db_error)?;
            return Ok(None);
        }
        let op = Self::operation(&tx, agent, field(&claim, "id")?, false).await?;
        let app = Self::app(&tx, field(&op, "app_id")?).await?;
        let cluster = rows(
            &tx,
            "SELECT to_jsonb(c) AS value FROM webdock_auth.hosting_cluster c WHERE id=$1",
            vec![agent.cluster.clone().into()],
        )
        .await?
        .pop()
        .ok_or_else(|| error(404, "Cluster unavailable."))?;
        let spec = if app["observed_spec"].is_object() {
            &app["observed_spec"]
        } else {
            &op["desired"]["spec"]
        };
        let volume = spec
            .get("volumeBytes")
            .map(amount)
            .transpose()?
            .unwrap_or(0);
        let legacy = cluster["runtime_kind"] == "kubernetes";
        let runtime = RuntimeSpec {
            autoscaling: spec
                .get("autoscaling")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| error(503, "Invalid persisted scaling policy."))?,
            version: 1,
            installation: field(&cluster, "runtime_installation")?.into(),
            project_id: webdock_core::id::RecordId::parse(field(&op, "project_id")?)
                .map_err(|_| error(400, "Invalid project."))?,
            resource_id: webdock_core::id::RecordId::parse(field(&op, "app_id")?)
                .map_err(|_| error(400, "Invalid app."))?,
            revision: app["observed_revision"]
                .as_u64()
                .filter(|r| *r > 0)
                .unwrap_or(numeric(&op, "target_revision")? as u64),
            image: field(spec, "image")?.into(),
            arguments: serde_json::from_value(spec["args"].clone())
                .map_err(|_| error(400, "Invalid arguments."))?,
            cpu_millicores: amount(&spec["cpuMillicores"])? as u32,
            memory_bytes: amount(&spec["memoryBytes"])?,
            ephemeral_bytes: amount(&spec["ephemeralBytes"])?,
            replicas: amount(&spec["replicas"])? as u16,
            port: amount(&spec["port"])? as u16,
            health_path: Some(field(spec, "healthPath")?.into()),
            health_host: spec["healthHost"].as_str().map(str::to_owned),
            volume: (volume > 0).then_some(RetainedVolume {
                mount_path: "/data".into(),
                size_bytes: Some(volume),
            }),
            hosting_binding: if legacy {
                Some(HostingBinding {
                    public_route: None,
                    cluster_id: webdock_core::id::RecordId::parse(&agent.cluster)
                        .map_err(|_| error(400, "Invalid cluster."))?,
                    quota: serde_json::from_value(claim["desired"]["quota"].clone())
                        .map_err(|_| error(503, "Read allowance unavailable."))?,
                    final_quota: serde_json::from_value(claim["desired"]["quota"].clone())
                        .map_err(|_| error(503, "Read allowance unavailable."))?,
                })
            } else {
                None
            },
            hosted_access: None,
            plugin: None,
            mounts: vec![],
        };
        let kind = match field(&op, "action")? {
            "apps.logs" => ReadKind::Logs,
            "apps.inspect" => ReadKind::Inspect,
            "apps.metrics" => ReadKind::Metrics,
            _ => return Err(error(400, "Unsupported read.")),
        };
        let mut command = ReadCommand {
            operation_id: field(&op, "id")?.into(),
            generation: numeric(&op, "generation")? as u64,
            lease_until: epoch(&op["lease_until"])
                .ok_or_else(|| error(409, "Lease unavailable."))? as u64,
            authorization: String::new(),
            kind,
            tail: if kind == ReadKind::Logs { 200 } else { 0 },
            spec: runtime,
            expected_runtime_id: app["observed_uid"].as_str().map(str::to_owned),
        };
        command.authorization = self.read_mac(agent, &command)?;
        command
            .validate()
            .map_err(|_| error(503, "Runtime read capability unavailable."))?;
        execute(&tx,"UPDATE webdock_auth.hosting_operation SET desired=jsonb_set(desired,'{nativeRead}',$2) WHERE id=$1",vec![command.operation_id.clone().into(),serde_json::to_value(&command).map_err(|_|error(503,"Read serialization unavailable."))?.into()]).await?;
        Self::verify_agent(&tx, agent).await?;
        if !self.accepted_authority(&tx, &op).await? {
            return Err(error(403, "Read authority changed."));
        }
        tx.commit().await.map_err(db_error)?;
        Ok(Some(command))
    }
    fn verify_read(&self, agent: &AgentIdentity, op: &Value, command: &ReadCommand) -> Result<()> {
        command
            .validate()
            .map_err(|_| error(400, "Invalid read command."))?;
        let expected: ReadCommand = serde_json::from_value(op["desired"]["nativeRead"].clone())
            .map_err(|_| error(409, "Read command unavailable."))?;
        if expected.operation_id != command.operation_id
            || expected.generation != command.generation
            || expected.lease_until != command.lease_until
            || op["generation"] != command.generation
            || expected
                .fingerprint()
                .map_err(|_| error(400, "Invalid read."))?
                != command
                    .fingerprint()
                    .map_err(|_| error(400, "Invalid read."))?
            || !bool::from(
                self.read_mac(agent, command)?
                    .as_bytes()
                    .ct_eq(command.authorization.as_bytes()),
            )
        {
            return Err(error(403, "Read command authority changed."));
        }
        Ok(())
    }
    async fn read_live(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        op: &Value,
        command: &ReadCommand,
    ) -> Result<Value> {
        Self::verify_agent(tx, agent).await?;
        self.verify_read(agent, op, command)?;
        Self::lease(op, command.generation as i64)?;
        if !self.accepted_authority(tx, op).await? {
            return Err(error(403, "Read authority changed."));
        }
        let app = Self::app(tx, field(op, "app_id")?).await?;
        if app["revision"] != op["target_revision"]
            || app["observed_uid"].as_str() != command.expected_runtime_id.as_deref()
        {
            return Err(error(409, "Read application ownership changed."));
        }
        Ok(app)
    }
    pub async fn guard_read(&self, agent: &AgentIdentity, command: &ReadCommand) -> Result<()> {
        let tx = self.agent_tx(agent).await?;
        let op = Self::operation(&tx, agent, &command.operation_id, false).await?;
        self.read_live(&tx, agent, &op, command).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(())
    }
    fn read_receipt(&self, command: &ReadCommand, result: &Value, outcome: &str) -> Result<Value> {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.rls_secret.as_bytes())
            .map_err(|_| error(503, "Read receipt unavailable."))?;
        mac.update(b"hosting-read-result-v1\0");
        mac.update(&serde_json::to_vec(&json!([outcome, result])).unwrap());
        Ok(
            json!({"requestSha256":command.fingerprint().map_err(|_|error(400,"Invalid read command."))?,"resultMac":hex::encode(mac.finalize().into_bytes()),"outcome":outcome}),
        )
    }
    /// Logs: {logs:string,truncated:bool}; Inspect: ObservedState; Metrics: ResourceMetrics.
    /// Raw logs never enter persistent storage. Exact retries compare a keyed receipt digest.
    pub async fn complete_read(
        &self,
        agent: &AgentIdentity,
        command: &ReadCommand,
        result: Value,
    ) -> Result<Value> {
        self.complete_read_inner(agent, command, result, None).await
    }
    pub async fn complete_read_with_receipt(
        &self,
        agent: &AgentIdentity,
        command: &ReadCommand,
        result: Value,
        receipt_sha256: &str,
    ) -> Result<Value> {
        self.complete_read_inner(agent, command, result, Some(receipt_sha256))
            .await
    }
    async fn complete_read_inner(
        &self,
        agent: &AgentIdentity,
        command: &ReadCommand,
        result: Value,
        receipt_sha256: Option<&str>,
    ) -> Result<Value> {
        if serde_json::to_vec(&result)
            .map_err(|_| error(400, "Invalid read result."))?
            .len()
            > 65536
        {
            return Err(error(400, "Read result too large."));
        }
        let receipt = self.read_receipt(command, &result, "succeeded")?;
        let tx = self.agent_tx(agent).await?;
        let op = Self::operation(&tx, agent, &command.operation_id, true).await?;
        self.verify_read(agent, &op, command)?;
        if op["status"] == "succeeded" {
            if op["result"]["nativeReadReceipt"] != receipt {
                return Err(error(409, "Read receipt changed."));
            }
            self.record_accepted_delivery(
                &tx,
                agent,
                DeliveryCommand::Read(command),
                receipt_sha256,
            )
            .await?;
            tx.commit().await.map_err(db_error)?;
            return Ok(json!({"id":command.operation_id,"status":"succeeded"}));
        }
        let _app = self.read_live(&tx, agent, &op, command).await?;
        let output = match command.kind {
            ReadKind::Logs => {
                let object = result
                    .as_object()
                    .filter(|o| {
                        o.len() == 2 && o.contains_key("logs") && o.contains_key("truncated")
                    })
                    .ok_or_else(|| error(400, "Invalid log result."))?;
                let logs = object["logs"]
                    .as_str()
                    .filter(|s| s.len() <= 12000)
                    .ok_or_else(|| error(400, "Log result exceeds bound."))?;
                let truncated = object["truncated"]
                    .as_bool()
                    .ok_or_else(|| error(400, "Invalid truncation flag."))?;
                let proof = json!({"appID":op["app_id"],"revision":op["target_revision"],"namespace":op["desired"]["namespace"],"uid":command.expected_runtime_id,"ready":true,"deleted":false,"replicas":command.spec.replicas,"logs":logs,"logsTruncated":truncated||logs.len()>=11996});
                self.complete_with_runtime(&tx,agent,&json!({"id":command.operation_id,"generation":command.generation,"outcome":"succeeded","proof":proof}),true,None).await?;
                // The retained completion has redacted current, prior, observed and historic env values.
                let safe = Self::app(&tx, field(&op, "app_id")?).await?;
                json!({"logs":safe["logs"],"truncated":truncated||logs.len()>=11996})
            }
            ReadKind::Inspect => {
                let value: ObservedState = serde_json::from_value(result.clone())
                    .map_err(|_| error(400, "Invalid inspection result."))?;
                if value.resource_id != command.spec.resource_id.to_string()
                    || value.observed_at < command.lease_until.saturating_sub(300)
                    || value.observed_at > now() as u64 + 30
                    || !(value.runtime_id == command.expected_runtime_id
                        || (value.phase == Phase::Absent && value.runtime_id.is_none()))
                {
                    return Err(error(409, "Inspection target changed."));
                }
                serde_json::to_value(value).unwrap()
            }
            ReadKind::Metrics => {
                let value: ResourceMetrics = serde_json::from_value(result.clone())
                    .map_err(|_| error(400, "Invalid metrics result."))?;
                if value.observed_at < command.lease_until.saturating_sub(300)
                    || value.observed_at > now() as u64 + 30
                    || (value.cpu_nanoseconds.is_none()
                        && value.cpu_nanocores.is_none()
                        && value.memory_bytes.is_none())
                {
                    return Err(error(409, "Metrics are stale or unavailable."));
                }
                serde_json::to_value(value).unwrap()
            }
        };
        execute(&tx,"UPDATE webdock_auth.hosting_operation SET status='succeeded',result=coalesce(result,'{}')||jsonb_build_object('nativeReadReceipt',$2::jsonb,'readResult',$3::jsonb) WHERE id=$1",vec![command.operation_id.clone().into(),receipt.into(),output.into()]).await?;
        if command.kind != ReadKind::Logs {
            self.audit(
                &tx,
                field(&op, "subject")?,
                &format!("{}.complete", field(&op, "action")?),
                field(&op, "app_id")?,
                "succeeded",
            )
            .await?;
        }
        Self::verify_agent(&tx, agent).await?;
        if !self.accepted_authority(&tx, &op).await? {
            return Err(error(403, "Read authority changed."));
        }
        self.record_accepted_delivery(&tx, agent, DeliveryCommand::Read(command), receipt_sha256)
            .await?;
        self.admit_completion_commit(&tx, &op, Some(command.lease_until), None)
            .await?;
        tx.commit().await.map_err(db_error)?;
        Ok(json!({"id":command.operation_id,"status":"succeeded"}))
    }
    pub async fn complete_read_failure(
        &self,
        agent: &AgentIdentity,
        command: &ReadCommand,
        error_code: &str,
    ) -> Result<Value> {
        self.complete_read_failure_inner(agent, command, error_code, None)
            .await
    }
    pub async fn complete_read_failure_with_receipt(
        &self,
        agent: &AgentIdentity,
        command: &ReadCommand,
        error_code: &str,
        receipt_sha256: &str,
    ) -> Result<Value> {
        self.complete_read_failure_inner(agent, command, error_code, Some(receipt_sha256))
            .await
    }
    async fn complete_read_failure_inner(
        &self,
        agent: &AgentIdentity,
        command: &ReadCommand,
        error_code: &str,
        receipt_sha256: Option<&str>,
    ) -> Result<Value> {
        if error_code.is_empty()
            || error_code.len() > 80
            || !error_code
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(error(400, "Invalid read failure."));
        }
        let receipt = self.read_receipt(command, &json!(error_code), "failed")?;
        let tx = self.agent_tx(agent).await?;
        let op = Self::operation(&tx, agent, &command.operation_id, true).await?;
        self.verify_read(agent, &op, command)?;
        if op["status"] == "failed" {
            if op["result"]["nativeReadReceipt"] != receipt {
                return Err(error(409, "Read failure receipt changed."));
            }
            self.record_accepted_delivery(
                &tx,
                agent,
                DeliveryCommand::Read(command),
                receipt_sha256,
            )
            .await?;
            tx.commit().await.map_err(db_error)?;
            return Ok(json!({"id":command.operation_id,"status":"failed"}));
        }
        self.read_live(&tx, agent, &op, command).await?;
        execute(&tx,"UPDATE webdock_auth.hosting_operation SET status='failed',result=jsonb_build_object('error','Application read failed.','nativeReadReceipt',$2::jsonb) WHERE id=$1",vec![command.operation_id.clone().into(),receipt.into()]).await?;
        if command.kind == ReadKind::Logs {
            execute(&tx,"UPDATE webdock_auth.hosting_app SET last_error=coalesce(last_error,'The application logs could not be read.') WHERE id=$1",vec![field(&op,"app_id")?.into()]).await?;
        }
        self.audit(
            &tx,
            field(&op, "subject")?,
            &format!("{}.complete", field(&op, "action")?),
            field(&op, "app_id")?,
            "failed",
        )
        .await?;
        Self::verify_agent(&tx, agent).await?;
        if !self.accepted_authority(&tx, &op).await? {
            return Err(error(403, "Read authority changed."));
        }
        self.record_accepted_delivery(&tx, agent, DeliveryCommand::Read(command), receipt_sha256)
            .await?;
        self.admit_completion_commit(&tx, &op, Some(command.lease_until), None)
            .await?;
        tx.commit().await.map_err(db_error)?;
        Ok(json!({"id":command.operation_id,"status":"failed"}))
    }
    pub async fn get_read(&self, actor: &Principal, id: &str) -> Result<crate::ReadView> {
        contracts::record_id(id)?;
        let tx = self.begin(actor).await?;
        Self::module(&tx, false, false).await?;
        let op = rows(
            &tx,
            "SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_operation o WHERE id=$1",
            vec![id.into()],
        )
        .await?
        .pop()
        .ok_or_else(|| error(404, "Read unavailable."))?;
        if !is_read_action(field(&op, "action")?) {
            return Err(error(404, "Read unavailable."));
        }
        let actor = HostingActor::from(actor);
        Self::authorize(
            &tx,
            &actor,
            Some(field(&op, "project_id")?),
            None,
            false,
            false,
            true,
        )
        .await?;
        let result = crate::ReadView::from_stored(
            id,
            field(&op, "action")?,
            op["status"].clone(),
            &op["result"],
        )?;
        tx.commit().await.map_err(db_error)?;
        Ok(result)
    }
}
