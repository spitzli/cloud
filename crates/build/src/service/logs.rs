//! Bounded durable logs and source-authorized cursors; no Kubernetes transport in browser routes.
use super::*;

fn phase(value: &str) -> Result<()> {
    if ![
        "provisioning",
        "cloning",
        "preparing",
        "building",
        "pushing",
    ]
    .contains(&value)
    {
        return Err(error("invalid_build_phase"));
    }
    Ok(())
}
impl BuildService {
    pub async fn append_kubernetes_log(
        &self,
        worker: &WorkerIdentity,
        build_id: &str,
        generation: i32,
        entry_key: &str,
        stage: &str,
        text: &str,
    ) -> Result<Value> {
        crate::contracts::id(build_id)?;
        crate::contracts::key(entry_key)?;
        phase(stage)?;
        if text.len() > 8192 {
            return Err(error("build_log_limit"));
        }
        let tx = self.worker_tx().await?;
        let b = Self::lease(&tx, worker, build_id, generation).await?;
        if b["execution_backend"] != "kubernetes" {
            return Err(error("invalid_execution_backend"));
        }
        let environment = self
            .hosting
            .git_build_environment(&tx, field(&b, "source_id")?)
            .await
            .map_err(hosting)?;
        let redactor = crate::execution::GuestRequest {
            recipe: "dockerfile".into(),
            root_directory: ".".into(),
            build_environment: environment,
            limits: crate::execution::Limits {
                cpu: 2,
                memory_mi_b: 4096,
                disk_bytes: 10 * 1024 * 1024 * 1024,
                duration_seconds: 1500,
            },
        };
        let text = redactor.redact(text);
        let receipt = hex::encode(Sha256::digest(
            serde_json::to_vec(&json!([stage, text])).map_err(|_| error("invalid_build_log"))?,
        ));
        let prior=rows(&tx,"SELECT jsonb_build_object('cursor',id::text,'receipt',receipt) AS value FROM webdock_auth.git_build_log_entry WHERE build_id=$1 AND generation=$2 AND entry_key=$3",vec![build_id.into(),generation.into(),entry_key.into()]).await?.pop();
        if let Some(prior) = prior {
            if prior["receipt"] != receipt {
                return Err(error("build_log_conflict"));
            }
            tx.commit().await.map_err(db_error)?;
            return Ok(json!({"cursor":prior["cursor"]}));
        }
        if execute(&tx,"UPDATE webdock_auth.git_build SET retained_log_bytes=retained_log_bytes+$2,phase=$3 WHERE id=$1 AND retained_log_bytes+$2<=1048576",vec![build_id.into(),(text.len() as i32).into(),stage.into()]).await?!=1 {return Err(error("build_log_limit"));}
        let entry=rows(&tx,"INSERT INTO webdock_auth.git_build_log_entry(build_id,customer_id,project_id,generation,entry_key,phase,text,receipt) VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING jsonb_build_object('cursor',id::text) AS value",vec![build_id.into(),field(&b,"customer_id")?.into(),field(&b,"project_id")?.into(),generation.into(),entry_key.into(),stage.into(),text.into(),receipt.into()]).await?.pop().ok_or_else(||error("build_log_unavailable"))?;
        tx.commit().await.map_err(db_error)?;
        Ok(entry)
    }
    pub(super) async fn log_page(
        tx: &DatabaseTransaction,
        project: &str,
        build: &str,
        after: &str,
        limit: u32,
    ) -> Result<Value> {
        crate::contracts::id(build)?;
        if after != "0" {
            crate::contracts::id(after)?;
        }
        if !(1..=100).contains(&limit) {
            return Err(error("invalid_pagination"));
        }
        let b=rows(tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b WHERE id=$1 AND project_id=$2",vec![build.into(),project.into()]).await?.pop().ok_or_else(||error("build_unavailable"))?;
        let entries=rows(tx,"SELECT jsonb_build_object('cursor',id::text,'generation',generation,'phase',phase,'text',text,'createdAt',created_at) AS value FROM webdock_auth.git_build_log_entry WHERE build_id=$1 AND id>$2::bigint ORDER BY id LIMIT $3",vec![build.into(),after.into(),i64::from(limit).into()]).await?;
        let cursor = entries
            .last()
            .and_then(|e| e["cursor"].as_str())
            .unwrap_or(after);
        Ok(
            json!({"entries":entries,"cursor":cursor,"status":b["status"],"phase":b["phase"],"terminal":matches!(b["status"].as_str(),Some("succeeded"|"failed"|"cancelled"))}),
        )
    }
}

impl BuildService {
    pub(super) async fn guard_kubernetes_publication(
        tx: &DatabaseTransaction,
        release: &Value,
    ) -> Result<()> {
        let b=rows(tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.git_build b JOIN webdock_auth.git_artifact a ON a.build_id=b.id WHERE a.id=$1 FOR UPDATE OF b",vec![field(release,"artifact_id")?.into()]).await?.pop().ok_or_else(||error("build_unavailable"))?;
        if b["execution_backend"] == "kubernetes"
            && !matches!(b["status"].as_str(), Some("artifact-ready" | "succeeded"))
        {
            return Err(error("build_publication_cancelled"));
        }
        Ok(())
    }
    pub(super) async fn mark_kubernetes_published(
        tx: &DatabaseTransaction,
        release: &Value,
    ) -> Result<()> {
        // RegistryPublisher has verified the actual digest before this existing staging callback.
        Self::guard_kubernetes_publication(tx, release).await?;
        execute(tx,"UPDATE webdock_auth.git_build SET status='succeeded',phase='succeeded',registry_verified_at=coalesce(registry_verified_at,now()),finished_at=coalesce(finished_at,now()) WHERE id=(SELECT build_id FROM webdock_auth.git_artifact WHERE id=$1) AND execution_backend='kubernetes' AND status IN ('artifact-ready','succeeded')",vec![field(release,"artifact_id")?.into()]).await?;
        Ok(())
    }
}
