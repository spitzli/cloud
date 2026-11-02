//! Application source guards. Mail keeps its existing epoch/installation transition path.
use super::*;
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AppGuardCheckpoint {
    app_guard: AppEffectGuard,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AppEffectGuard {
    phase: AppEffectPhase,
    fence_fingerprint: String,
}
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum AppEffectPhase {
    Preflight,
    Stop,
    Capture,
    Upload,
    Resume,
    Release,
}
impl AppEffectPhase {
    fn name(self) -> &'static str {
        match self {
            Self::Preflight => "preflight",
            Self::Stop => "stop",
            Self::Capture => "capture",
            Self::Upload => "upload",
            Self::Resume => "resume",
            Self::Release => "release",
        }
    }
}
pub(super) fn app_effect_guard(c: &RecoveryClaim) -> Result<AppEffectGuard> {
    let value = c.checkpoint.as_ref().ok_or_else(failure)?;
    if serde_json::to_vec(value).map_err(|_| failure())?.len() > 256 {
        return Err(failure());
    }
    let input: AppGuardCheckpoint = serde_json::from_value(value.clone()).map_err(|_| failure())?;
    if input.app_guard.fence_fingerprint.len() != 64
        || !input
            .app_guard
            .fence_fingerprint
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(failure());
    }
    Ok(input.app_guard)
}
impl HostingService {
    pub(in crate::service) async fn ensure_app_effect_allowed(
        tx: &DatabaseTransaction,
        app: &str,
        committed: Option<&Value>,
    ) -> Result<()> {
        let allowed = one(
            tx,
            "SELECT webdock_rls.recovery_app_effect_allowed($1,$2,$3,$4) AS allowed",
            vec![
                app.into(),
                committed.and_then(|v| v["id"].as_str()).into(),
                committed
                    .and_then(|v| v["generation"].as_i64())
                    .map(|v| v as i32)
                    .into(),
                committed.and_then(|v| v["payload_hash"].as_str()).into(),
            ],
        )
        .await?
        .ok_or_else(failure)?
        .try_get::<bool>("", "allowed")
        .map_err(db_error)?;
        if !allowed {
            return Err(error(
                409,
                "Application recovery source is fenced. Reconciliation is required.",
            ));
        }
        Ok(())
    }
    pub(super) async fn begin_app_checkpoint(
        &self,
        agent: &AgentIdentity,
    ) -> Result<DatabaseTransaction> {
        // Deliberately no verify_agent row lock here: app admission first takes epoch NOWAIT,
        // plan, cluster and app. The exact enrolled agent is revalidated before mutation.
        let tx = self.worker.begin().await.map_err(db_error)?;
        one(&tx,"SELECT set_config('lock_timeout','5s',true),set_config('statement_timeout','10s',true)",vec![]).await?;
        self.apply_scope(&tx,"webdock_worker_runtime",json!({"kind":"system","purpose":"agent","clusterID":agent.cluster,"authProvider":"better-auth"})).await?;
        Ok(tx)
    }
    pub(super) async fn lock_app_source(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
    ) -> Result<()> {
        let allowed = one(
            tx,
            "SELECT webdock_rls.recovery_app_source_lock($1,$2,$3,$4::uuid,$5) AS allowed",
            app_source_parameters(agent, c),
        )
        .await?
        .ok_or_else(failure)?
        .try_get::<bool>("", "allowed")
        .map_err(db_error)?;
        if !allowed {
            return Err(failure());
        }
        Self::verify_agent(tx, agent).await?;
        Ok(())
    }
    pub(super) async fn begin_recovery_guarded(
        &self,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
    ) -> Result<DatabaseTransaction> {
        if !app_backup(c) {
            return self.begin_authenticated_agent(agent).await;
        }
        c.validate().map_err(|_| failure())?;
        if c.spec.cluster_id != agent.cluster {
            return Err(failure());
        }
        let tx = self.begin_app_checkpoint(agent).await?;
        self.guard_app_source(&tx, agent, c).await?;
        Ok(tx)
    }
    pub(super) async fn guard_app_source(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
    ) -> Result<()> {
        let allowed = one(
            tx,
            "SELECT webdock_rls.recovery_app_source_guard($1,$2,$3,$4::uuid,$5) AS allowed",
            app_source_parameters(agent, c),
        )
        .await?
        .ok_or_else(failure)?
        .try_get::<bool>("", "allowed")
        .map_err(db_error)?;
        if !allowed {
            return Err(failure());
        }
        Self::verify_agent(tx, agent).await?;
        Ok(())
    }
    pub(super) async fn guard_app_effect(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
        effect: AppEffectGuard,
    ) -> Result<()> {
        let mut values = app_source_parameters(agent, c);
        values.push(effect.phase.name().into());
        values.push(effect.fence_fingerprint.into());
        if !one(
            tx,
            "SELECT webdock_rls.recovery_app_effect_guard($1,$2,$3,$4::uuid,$5,$6,$7) AS allowed",
            values,
        )
        .await?
        .ok_or_else(failure)?
        .try_get::<bool>("", "allowed")
        .map_err(db_error)?
        {
            return Err(failure());
        }
        Ok(())
    }
    pub(super) async fn app_source_checkpoint(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
        checkpoint: &Value,
    ) -> Result<()> {
        let mut values = app_source_parameters(agent, c);
        values.push(
            if checkpoint["phase"] == "app-admit" {
                checkpoint["participants"].clone()
            } else {
                checkpoint.clone()
            }
            .into(),
        );
        let (sql, boundary, audit) = match checkpoint["phase"].as_str() {
            Some("app-admit") => (
                "SELECT webdock_rls.recovery_app_source_admit($1,$2,$3,$4::uuid,$5,$6,$7,$8) IS NOT NULL AS allowed",
                RecoveryHistoryBoundary::AppAdmit,
                true,
            ),
            Some("app-stopped" | "app-captured") => (
                "SELECT webdock_rls.recovery_app_source_drained($1,$2,$3,$4::uuid,$5,$6,$7) AS allowed",
                RecoveryHistoryBoundary::AppDrain,
                false,
            ),
            Some("app-resume-intent" | "app-resumed") => (
                "SELECT webdock_rls.recovery_app_source_resume($1,$2,$3,$4::uuid,$5,$6,$7,$8) AS allowed",
                RecoveryHistoryBoundary::AppResume,
                true,
            ),
            _ => return Ok(()),
        };
        values.push(self.id().await?.into());
        if audit {
            values.push(self.id().await?.into());
        }
        if !recovery_history_one(tx, sql, values, boundary)
            .await?
            .ok_or_else(failure)?
            .try_get::<bool>("", "allowed")
            .map_err(db_error)?
        {
            return Err(failure());
        }
        Ok(())
    }
    pub(super) async fn app_source_plan(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
    ) -> Result<Value> {
        one(
            tx,
            "SELECT webdock_rls.recovery_app_source_view($1,$2,$3,$4::uuid,$5) AS value",
            app_source_parameters(agent, c),
        )
        .await?
        .map(object)
        .transpose()?
        .ok_or_else(failure)
    }
}
pub(super) fn app_backup(c: &RecoveryClaim) -> bool {
    c.spec.profile == RecoveryProfile::NtfySqliteOfflineV1
        && c.spec.action == RecoveryAction::Backup
}
fn app_source_parameters(agent: &AgentIdentity, c: &RecoveryClaim) -> Vec<sea_orm::Value> {
    vec![
        c.spec.operation_id.clone().into(),
        (c.lease.generation as i32).into(),
        digest(&c.lease.token).into(),
        c.spec.epoch.clone().into(),
        agent.generation.into(),
    ]
}
#[cfg(test)]
mod review_source_contracts {
    use super::*;
    #[test]
    fn normal_effect_guards_share_app_lock_but_source_admission_stays_exclusive() {
        let sql = include_str!("../../persistence/migrations/0054_source_recovery_fences.sql");
        let normal = sql
            .split("CREATE FUNCTION webdock_rls.recovery_app_effect_allowed(")
            .nth(1)
            .unwrap()
            .split("END $$;")
            .next()
            .unwrap();
        assert!(
            normal.contains("WHERE id=app FOR SHARE NOWAIT;"),
            "normal read guards unnecessarily exclude each other"
        );
        assert!(!normal.contains("FOR UPDATE"));
        let source = sql
            .split("CREATE FUNCTION webdock_rls.recovery_app_source_lock(")
            .nth(1)
            .unwrap()
            .split("END $$;")
            .next()
            .unwrap();
        assert!(source.contains("WHERE id=p.source_app_id FOR UPDATE NOWAIT;"));
    }
    // These check the literal guard/transaction contract, not PostgreSQL execution.
    // Missing/null outcome, epoch contention and state/event proofs run in the genuine fixture.
    #[test]
    fn app_resume_rejects_the_entire_nullable_transition_unless_true() {
        let sql = include_str!("../../persistence/migrations/0054_source_recovery_fences.sql");
        let function = sql
            .split("CREATE FUNCTION webdock_rls.recovery_app_source_resume(")
            .nth(1)
            .unwrap()
            .split("END $$;")
            .next()
            .unwrap();
        assert!(
            function.contains(") IS NOT TRUE THEN RETURN false;END IF;"),
            "nullable SQL predicate may fall through to release"
        );
    }
    #[test]
    fn app_periodic_guard_uses_epoch_first_transaction_and_canonical_phase() {
        let source = include_str!("recovery.rs");
        let guard = source
            .split("pub async fn guard_recovery(")
            .nth(1)
            .unwrap()
            .split("///")
            .next()
            .unwrap();
        assert!(
            guard.contains("begin_recovery_guarded"),
            "periodic guard starts with agent row lock instead of epoch"
        );
        let sql = include_str!("../../persistence/migrations/0054_source_recovery_fences.sql");
        let function = sql
            .split("CREATE FUNCTION webdock_rls.recovery_app_source_guard(")
            .nth(1)
            .expect("specialized app guard absent")
            .split("END $$;")
            .next()
            .unwrap();
        assert!(function.contains("recovery_app_source_lock(operation,generation,token_hash,expected_epoch,agent_generation)"));
        assert!(function.contains("f.epoch IS DISTINCT FROM expected_epoch"));
        assert!(
            function.contains("j.checkpoint->>'fenceFingerprint' IS DISTINCT FROM f.plan_sha256")
        );
    }
    #[test]
    fn normal_core_mutations_advance_revision_and_reconcile_advances_only_operation_generation() {
        let apps = include_str!("apps.rs");
        assert!(apps.contains("active.revision = Set(if model.lifecycle == \"draft\""));
        assert!(apps.contains(".checked_add(1)"));
        let reconcile = apps
            .split("if action == \"apps.reconcile\"")
            .nth(1)
            .unwrap()
            .split("let busy=")
            .next()
            .unwrap();
        assert!(reconcile.contains("UPDATE webdock_auth.hosting_operation SET status='queued'"));
        assert!(!reconcile.contains("active.revision"));
        assert!(include_str!("agent.rs").contains("SET status='running',generation=generation+1,lease_until=now()+interval '300 seconds' WHERE id=$1"));
        assert!(
            include_str!("runtime.rs")
                .contains("revision: numeric(op, \"target_revision\")? as u64")
        );
    }
    fn claim(checkpoint: Value) -> RecoveryClaim {
        serde_json::from_value(json!({"spec":{"version":1,"operationID":"123","policyID":"124","policyRevision":1,"clusterID":"125","profile":"ntfy-sqlite-offline-v1","action":"backup","source":{},"target":null,"recoverySetID":null,"epoch":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","capacityBytes":67108864},"lease":{"generation":1,"token":"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb","expiresAt":"2099-01-01T00:00:00Z"},"checkpoint":checkpoint})).unwrap()
    }
    #[test]
    fn app_effect_guard_envelope_has_no_missing_null_unknown_or_extra_fields() {
        for phase in [
            "preflight",
            "stop",
            "capture",
            "upload",
            "resume",
            "release",
        ] {
            let c = claim(json!({"appGuard":{"phase":phase,"fenceFingerprint":"a".repeat(64)}}));
            assert_eq!(app_effect_guard(&c).unwrap().phase.name(), phase);
        }
        for invalid in [
            Value::Null,
            json!({}),
            json!({"appGuard":null}),
            json!({"appGuard":{"phase":"unknown","fenceFingerprint":"a".repeat(64)}}),
            json!({"appGuard":{"phase":null,"fenceFingerprint":"a".repeat(64)}}),
            json!({"appGuard":{"fenceFingerprint":"a".repeat(64)}}),
            json!({"appGuard":{"phase":"stop","fenceFingerprint":null}}),
            json!({"appGuard":{"phase":"stop","fenceFingerprint":"A".repeat(64)}}),
            json!({"appGuard":{"phase":"stop","fenceFingerprint":"a".repeat(64),"extra":true}}),
            json!({"appGuard":{"phase":"stop","fenceFingerprint":"a".repeat(64)},"extra":true}),
        ] {
            assert!(app_effect_guard(&claim(invalid)).is_err());
        }
    }
    #[test]
    fn app_provider_phase_table_covers_all_36_pairs_and_denies_canonical_advance() {
        let sql = include_str!("../../persistence/migrations/0054_source_recovery_fences.sql");
        let body = sql
            .split("CREATE FUNCTION webdock_rls.recovery_app_effect_guard(")
            .nth(1)
            .unwrap()
            .split("END $$;")
            .next()
            .unwrap();
        let regex = regex::Regex::new(r"\('([^']+)','([^']+)','([^']+)'\)").unwrap();
        let rows = regex
            .captures_iter(body)
            .map(|c| (c[1].to_owned(), c[2].to_owned(), c[3].to_owned()))
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 6);
        let phases = [
            "preflight",
            "stop",
            "capture",
            "upload",
            "resume",
            "release",
        ];
        let states = [
            ("draining", "app-admit"),
            ("stopped", "app-stopped"),
            ("captured", "app-captured"),
            ("resuming", "app-resume-intent"),
            ("released", "app-resumed"),
            ("uncertain", ""),
        ];
        for (i, phase) in phases.iter().enumerate() {
            for (j, (state, checkpoint)) in states.iter().enumerate() {
                let allowed = rows
                    .iter()
                    .any(|r| r.0 == *phase && r.1 == *state && r.2 == *checkpoint);
                assert_eq!(
                    allowed,
                    j == i.saturating_sub(1),
                    "phase={phase}, state={state}"
                );
            }
        }
        assert!(
            !rows
                .iter()
                .any(|r| r.0 == "upload" && r.1 == "captured" && r.2 == "uploaded-verified")
        );
        assert!(
            !rows
                .iter()
                .any(|r| r.0 == "capture" && r.1 == "resuming" && r.2 == "app-resume-intent")
        );
        assert!(body.contains("providerOutcome'='confirmed-complete'"));
        assert!(body.contains("runtimeUID'=f.source_observed_uid"));
    }
}
