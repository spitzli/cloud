//! Durable fixed-volume authority is separate from execution and publication leases.
use super::*;
use crate::KubernetesStoragePolicy;
use spitzli_agent_protocol::build_storage::{
    BuildStorageAction, BuildStorageReservation, BuildStorageTarget, BuildVolumeHandle,
};

impl BuildService {
    pub async fn configure_kubernetes_storage(
        &self,
        actor: &Principal,
        cluster_id: &str,
        policy: Option<KubernetesStoragePolicy>,
        runtime_fingerprint: Option<&str>,
    ) -> Result<()> {
        crate::contracts::id(cluster_id)?;
        if policy.is_some() != runtime_fingerprint.is_some() {
            return Err(error("invalid_build_storage_policy"));
        }
        if let Some(policy) = &policy {
            policy.validate(cluster_id)?;
        }
        if runtime_fingerprint.is_some_and(|f| {
            f.len() != 64
                || !f
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) {
            return Err(error("invalid_kubernetes_fingerprint"));
        }
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| error("authentication_required"))?;
        self.operator(&tx, actor).await?;
        Self::kubernetes_admission_lock(&tx).await?;
        let cluster=rows(&tx,"SELECT to_jsonb(c) AS value FROM webdock_auth.git_build_cluster c WHERE id=$1 FOR UPDATE",vec![cluster_id.into()]).await?.pop().ok_or_else(||error("kubernetes_cluster_unavailable"))?;
        if !cluster["storage_policy"].is_null() && policy.is_none() {
            return Err(error("kubernetes_storage_downgrade_forbidden"));
        }
        let live=rows(&tx,"SELECT to_jsonb(count(*)) AS value FROM webdock_auth.git_build_attempt WHERE cluster_id=$1 AND retired_at IS NULL",vec![cluster_id.into()]).await?.pop().and_then(|v|v.as_i64()).unwrap_or(1);
        if cluster["enabled"] == true || live != 0 {
            return Err(error("kubernetes_cluster_must_be_drained"));
        }
        let policy = policy
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| error("invalid_build_storage_policy"))?;
        execute(&tx,"UPDATE webdock_auth.git_build_cluster SET storage_policy=$2,storage_runtime_fingerprint=$3 WHERE id=$1",vec![cluster_id.into(),policy.into(),runtime_fingerprint.map(str::to_owned).into()]).await?;
        self.audit(
            &tx,
            &actor.user_id,
            "git.kubernetes.storage",
            cluster_id,
            "explicit-policy-change",
        )
        .await?;
        self.operator(&tx, actor).await?;
        tx.commit().await.map_err(db_error)
    }
    pub(super) async fn assign_storage_request(
        &self,
        tx: &DatabaseTransaction,
        cluster: &Value,
        build: &Value,
        attempt_id: &str,
        generation: i32,
    ) -> Result<Value> {
        if cluster["storage_policy"].is_null() {
            return Ok(Value::Null);
        }
        let policy: KubernetesStoragePolicy =
            serde_json::from_value(cluster["storage_policy"].clone())
                .map_err(|_| error("invalid_build_storage_policy"))?;
        policy.validate(field(cluster, "id")?)?;
        let request = BuildStorageReservation {
            version: 1,
            audience: "webdock-build-storage-v1".into(),
            volume_id: self.id().await?,
            customer_id: field(build, "customer_id")?.into(),
            project_id: field(build, "project_id")?.into(),
            build_id: field(build, "id")?.into(),
            generation: generation
                .try_into()
                .map_err(|_| error("invalid_generation"))?,
            target: BuildStorageTarget {
                cluster_id: field(cluster, "id")?.into(),
                node: policy.node,
                pool_id: policy.pool_id,
            },
            volume_bytes: policy.volume_bytes,
            action: BuildStorageAction::Reserve,
            epoch: 1,
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(240),
        };
        request
            .validate_at(chrono::Utc::now())
            .map_err(|_| error("invalid_build_storage_authority"))?;
        let value =
            serde_json::to_value(request).map_err(|_| error("invalid_build_storage_authority"))?;
        execute(tx,"UPDATE webdock_auth.git_build_attempt SET storage_request=$2,storage_epoch=1 WHERE id=$1",vec![attempt_id.into(),value.clone().into()]).await?;
        Ok(value)
    }
    async fn storage_attempt(
        tx: &DatabaseTransaction,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
    ) -> Result<Value> {
        Self::verify(tx, worker).await?;
        rows(
            tx,
            "SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b WHERE id=$1 FOR UPDATE",
            vec![build_id.into()],
        )
        .await?;
        rows(tx,"SELECT to_jsonb(a) AS value FROM webdock_auth.git_build_attempt a JOIN webdock_auth.git_build_cluster c ON c.id=a.cluster_id WHERE a.build_id=$1 AND a.generation=$2 AND c.worker_id=$3 AND c.worker_generation=$4 FOR UPDATE OF a",vec![build_id.into(),generation.into(),worker.id.clone().into(),worker.generation.into()]).await?.pop().ok_or_else(||error("kubernetes_attempt_lost"))
    }
    pub async fn issue_kubernetes_storage(
        &self,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
        action: BuildStorageAction,
    ) -> Result<BuildStorageReservation> {
        crate::contracts::id(build_id)?;
        let tx = self.worker_tx().await?;
        if action != BuildStorageAction::Retire {
            Self::lease(&tx, worker, build_id, generation).await?;
        }
        let attempt = Self::storage_attempt(&tx, worker, build_id, generation).await?;
        if !attempt["retired_at"].is_null()
            || (attempt["storage_closing"] == true && action != BuildStorageAction::Retire)
        {
            return Err(error("kubernetes_volume_fenced"));
        }
        let mut request: BuildStorageReservation =
            serde_json::from_value(attempt["storage_request"].clone())
                .map_err(|_| error("kubernetes_storage_unconfigured"))?;
        let epoch = attempt["storage_epoch"]
            .as_u64()
            .and_then(|v| v.checked_add(1))
            .filter(|v| *v <= 9007199254740991)
            .ok_or_else(|| error("generation_exhausted"))?;
        request.action = action;
        request.epoch = epoch;
        request.expires_at = chrono::Utc::now() + chrono::Duration::seconds(240);
        request
            .validate_at(chrono::Utc::now())
            .map_err(|_| error("invalid_build_storage_authority"))?;
        execute(&tx,"UPDATE webdock_auth.git_build_attempt SET storage_epoch=$3,storage_closing=storage_closing OR $4,cancel_requested=cancel_requested OR $4 WHERE build_id=$1 AND generation=$2",vec![build_id.into(),generation.into(),(epoch as i64).into(),(action==BuildStorageAction::Retire).into()]).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(request)
    }
    pub async fn record_kubernetes_volume(
        &self,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
        handle: BuildVolumeHandle,
    ) -> Result<()> {
        let tx = self.worker_tx().await?;
        Self::lease(&tx, worker, build_id, generation).await?;
        let attempt = Self::storage_attempt(&tx, worker, build_id, generation).await?;
        let request: BuildStorageReservation =
            serde_json::from_value(attempt["storage_request"].clone())
                .map_err(|_| error("kubernetes_storage_unconfigured"))?;
        handle
            .validate_for(&request)
            .map_err(|_| error("kubernetes_volume_conflict"))?;
        if handle.namespace != field(&attempt, "namespace")? || attempt["storage_closing"] == true {
            return Err(error("kubernetes_volume_fenced"));
        }
        let value =
            serde_json::to_value(handle).map_err(|_| error("invalid_build_storage_handle"))?;
        if execute(&tx,"UPDATE webdock_auth.git_build_attempt SET volume_handle=$3 WHERE build_id=$1 AND generation=$2 AND retired_at IS NULL AND NOT cancel_requested AND (volume_handle IS NULL OR volume_handle=$3) AND (spec_fingerprint IS NULL OR volume_handle=$3)",vec![build_id.into(),generation.into(),value.into()]).await?!=1 {return Err(error("kubernetes_volume_conflict"));}
        tx.commit().await.map_err(db_error)
    }
    pub async fn record_kubernetes_volume_retired(
        &self,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
        epoch: u64,
    ) -> Result<()> {
        let tx = self.worker_tx().await?;
        let attempt = Self::storage_attempt(&tx, worker, build_id, generation).await?;
        if attempt["storage_request"].is_null()
            || attempt["storage_closing"] != true
            || attempt["storage_epoch"].as_u64() != Some(epoch)
        {
            return Err(error("kubernetes_volume_epoch_conflict"));
        }
        if attempt["storage_retired_at"].is_null() {
            execute(
                &tx,
                "UPDATE webdock_auth.git_build_attempt SET storage_retired_at=now() WHERE id=$1",
                vec![field(&attempt, "id")?.into()],
            )
            .await?;
        }
        tx.commit().await.map_err(db_error)
    }
}
