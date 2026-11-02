//! Bounded reads of actual completed Core logs, never caller-submitted diagnostics.
use super::*;
use sha2::{Digest, Sha256};

fn snapshot(scope: &InvocationScope, row: &Value) -> Result<(SourceRef, Vec<u8>)> {
    let logs = row["logs"]
        .as_str()
        .ok_or_else(|| error("build_log_unavailable"))?;
    let id = row["id"]
        .as_str()
        .ok_or_else(|| error("build_log_unavailable"))?;
    let hash = row["completion_hash"].as_str().unwrap_or("");
    let finished = row["finished_at"].as_str().unwrap_or("");
    if scope.plugin_id != "archives"
        || row["customer_id"] != scope.customer_id
        || row["project_id"] != scope.project_id
        || !webdock_export_protocol::decimal(id)
        || !matches!(row["status"].as_str(), Some("succeeded" | "failed"))
        || row["generation"].as_i64().is_none_or(|n| n < 1)
        || hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || chrono::DateTime::parse_from_rfc3339(finished).is_err()
        || logs.len() > webdock_export_protocol::MAX_BUILD_LOG_BYTES
        || row["source_enabled"] != true
        || row["connection_state"] != "active"
    {
        return Err(error("build_log_unavailable"));
    }
    let binding = fingerprint(&json!([
        "core-stored-buildlog-v1",
        scope.customer_id,
        scope.project_id,
        id,
        row["source_id"],
        row["source_sha"],
        row["generation"],
        row["status"],
        finished,
        hash,
        row["source_revision"],
        row["connection_generation"],
        row["environment_revision"],
        row["environment_identity"],
        row["worker_id"],
        row["worker_generation"],
        row["subject"],
        row["current_source_revision"],
        row["current_environment_revision"],
        row["current_connection_id"],
        row["current_connection_generation"],
        row["hosting_revision"],
        hex::encode(Sha256::digest(logs.as_bytes()))
    ]))
    .map_err(|_| error("build_log_unavailable"))?;
    Ok((
        SourceRef {
            version: 1,
            kind: SourceKind::BuildLog,
            id: id.into(),
            binding,
            commit: None,
        },
        logs.as_bytes().to_vec(),
    ))
}

impl BuildService {
    async fn export_log_access(scope: &InvocationScope, tx: &DatabaseTransaction) -> Result<()> {
        Self::module(tx, true).await?;
        let access=rows(tx,"SELECT jsonb_build_object('subject',webdock_rls.actor()->>'subject','session',webdock_rls.actor()->>'sessionID','allowed',webdock_rls.customer_permission(p.customer_id,'org:deployments:create'),'mode',h.mode,'operator',webdock_rls.operator() AND EXISTS(SELECT 1 FROM webdock_admin.users u WHERE u.auth_subject=webdock_rls.actor()->>'subject' AND u.role='operator')) AS value FROM webdock_admin.projects p JOIN webdock_admin.customers c ON c.id=p.customer_id JOIN webdock_auth.hosting_project h ON h.project_id=p.id AND h.customer_id=c.id WHERE p.id=$1 AND p.customer_id=$2 AND p.status='active' AND c.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted')",vec![scope.project_id.clone().into(),scope.customer_id.clone().into()]).await?.pop().ok_or_else(||error("permission_denied"))?;
        if scope.plugin_id != "archives"
            || access["subject"] != scope.subject
            || access["session"] != scope.session_id
            || access["allowed"] != true
            || (access["mode"] != "selfservice" && access["operator"] != true)
        {
            return Err(error("permission_denied"));
        }
        Ok(())
    }
    async fn export_log_row(
        scope: &InvocationScope,
        tx: &DatabaseTransaction,
        id: &str,
    ) -> Result<Value> {
        if !webdock_export_protocol::decimal(id) {
            return Err(error("invalid_build"));
        }
        Self::export_log_access(scope, tx).await?;
        rows(tx,"SELECT to_jsonb(b)||jsonb_build_object('current_source_revision',s.revision,'current_environment_revision',s.environment_revision,'source_enabled',s.enabled,'connection_state',c.state,'current_connection_id',c.id,'current_connection_generation',c.generation,'hosting_revision',h.revision) AS value FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id AND s.customer_id=b.customer_id AND s.project_id=b.project_id JOIN webdock_auth.git_connection c ON c.id=s.connection_id AND c.customer_id=b.customer_id JOIN webdock_auth.hosting_project h ON h.project_id=b.project_id AND h.customer_id=b.customer_id WHERE b.id=$1 AND b.customer_id=$2 AND b.project_id=$3 AND b.status IN ('succeeded','failed') AND b.finished_at IS NOT NULL AND b.completion_hash~'^[a-f0-9]{64}$' AND octet_length(b.logs)<=24000 AND s.enabled AND c.state='active'",vec![id.into(),scope.customer_id.clone().into(),scope.project_id.clone().into()]).await?.pop().ok_or_else(||error("build_log_unavailable"))
    }
    pub async fn export_log_reference(
        scope: &InvocationScope,
        tx: &DatabaseTransaction,
        id: &str,
    ) -> Result<SourceRef> {
        let row = Self::export_log_row(scope, tx, id).await?;
        snapshot(scope, &row).map(|v| v.0)
    }
    pub async fn export_log_bytes(
        scope: &InvocationScope,
        tx: &DatabaseTransaction,
        source: &SourceRef,
    ) -> Result<Vec<u8>> {
        source.finished().map_err(|_| error("invalid_source"))?;
        if source.kind != SourceKind::BuildLog {
            return Err(error("invalid_source"));
        }
        let row = Self::export_log_row(scope, tx, &source.id).await?;
        let (current, bytes) = snapshot(scope, &row)?;
        if current != *source {
            return Err(error("build_log_changed"));
        }
        Ok(bytes)
    }
    pub async fn export_log_choices(
        scope: &InvocationScope,
        tx: &DatabaseTransaction,
    ) -> Result<Vec<Value>> {
        if Self::export_log_access(scope, tx).await.is_err() {
            return Ok(vec![]);
        }
        rows(tx,"SELECT jsonb_build_object('id',b.id,'kind','buildlog','state','ready') AS value FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id AND s.customer_id=b.customer_id AND s.project_id=b.project_id JOIN webdock_auth.git_connection c ON c.id=s.connection_id AND c.customer_id=b.customer_id WHERE b.project_id=$1 AND b.customer_id=$2 AND b.status IN ('succeeded','failed') AND b.finished_at IS NOT NULL AND b.completion_hash~'^[a-f0-9]{64}$' AND octet_length(b.logs)<=24000 AND s.enabled AND c.state='active' ORDER BY b.finished_at DESC,b.id DESC LIMIT 100",vec![scope.project_id.clone().into(),scope.customer_id.clone().into()]).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope() -> InvocationScope {
        InvocationScope {
            plugin_id: "archives".into(),
            customer_id: "123".into(),
            project_id: "124".into(),
            subject: "member".into(),
            session_id: "session".into(),
            operation_id: "actual-export".into(),
            generation: 1,
            grant_revision: 0,
        }
    }
    fn row() -> Value {
        json!({"id":"125","customer_id":"123","project_id":"124","source_id":"126","source_sha":"a".repeat(40),"generation":2,"status":"succeeded","finished_at":"2026-10-25T12:00:00+00:00","completion_hash":"b".repeat(64),"logs":"actual captured build output\n","source_revision":1,"connection_generation":1,"environment_revision":1,"environment_identity":"c".repeat(64),"worker_id":"enrolled-worker","worker_generation":1,"subject":"builder","current_source_revision":1,"current_environment_revision":1,"source_enabled":true,"connection_state":"active","current_connection_id":"127","current_connection_generation":1,"hosting_revision":1})
    }
    #[test]
    fn completed_stored_success_and_failure_are_actual_bytes_with_content_bound_identity() {
        let original = row();
        let (reference, bytes) = snapshot(&scope(), &original).unwrap();
        assert_eq!(bytes, b"actual captured build output\n");
        assert_eq!(reference.kind, SourceKind::BuildLog);
        assert!(
            !serde_json::to_string(&reference)
                .unwrap()
                .contains("captured")
        );
        for (field, value) in [
            ("logs", json!("same generation changed bytes")),
            ("completion_hash", json!("d".repeat(64))),
            ("generation", json!(3)),
            ("status", json!("failed")),
            ("finished_at", json!("2026-10-25T12:01:00+00:00")),
            ("current_source_revision", json!(2)),
            ("current_connection_generation", json!(2)),
            ("current_environment_revision", json!(2)),
            ("hosting_revision", json!(2)),
        ] {
            let mut changed = original.clone();
            changed[field] = value;
            assert_ne!(
                snapshot(&scope(), &changed).unwrap().0.binding,
                reference.binding,
                "{field} must revoke retained source"
            );
        }
    }
    #[test]
    fn unfinished_foreign_unreceipted_disabled_and_oversized_logs_are_denied() {
        for (field, value) in [
            ("status", json!("running")),
            ("status", json!("queued")),
            ("status", json!("cancelled")),
            ("customer_id", json!("999")),
            ("project_id", json!("999")),
            ("finished_at", Value::Null),
            ("completion_hash", Value::Null),
            ("generation", json!(0)),
            ("source_enabled", json!(false)),
            ("connection_state", json!("disabled")),
            ("logs", json!("x".repeat(24_001))),
        ] {
            let mut changed = row();
            changed[field] = value;
            assert!(
                snapshot(&scope(), &changed).is_err(),
                "{field} must be denied"
            );
        }
        let mut boundary = row();
        boundary["logs"] = json!("é".repeat(12_000));
        assert_eq!(snapshot(&scope(), &boundary).unwrap().1.len(), 24_000);
        boundary["logs"] = json!("é".repeat(12_001));
        assert!(snapshot(&scope(), &boundary).is_err());
    }
}
