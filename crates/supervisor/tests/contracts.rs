#![cfg(feature = "journal")]
use spitzli_core::id::RecordId;
use spitzli_supervisor::*;
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
fn spec() -> RuntimeSpec {
    RuntimeSpec {
        autoscaling: None,
        version: 1,
        installation: "test-install".into(),
        project_id: RecordId::parse("123").unwrap(),
        resource_id: RecordId::parse("456").unwrap(),
        revision: 1,
        image: format!("docker.io/library/busybox@sha256:{}", "a".repeat(64)),
        arguments: vec!["httpd".into(), "-f".into(), "-p".into(), "8080".into()],
        cpu_millicores: 100,
        memory_bytes: 32 * 1024 * 1024,
        ephemeral_bytes: 16 * 1024 * 1024,
        replicas: 1,
        port: 8080,
        health_path: Some("/".into()),
        health_host: None,
        volume: None,
        plugin: None,
        mounts: Vec::new(),
        hosting_binding: None,
        hosted_access: None,
    }
}
fn command(generation: u64) -> Command {
    Command {
        operation_id: format!("operation-{generation}"),
        generation,
        lease_until: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 300,
        authorization: "test-only-reference".into(),
        material: None,
        action: Action::Apply,
        spec: spec(),
        expected_runtime_id: None,
    }
}
#[test]
fn optional_probe_host_preserves_old_runtime_bytes_and_rejects_non_authorities() {
    let old = serde_json::to_value(spec()).unwrap();
    assert!(old.get("healthHost").is_none());
    let restored: RuntimeSpec = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(
        serde_json::to_vec(&restored).unwrap(),
        serde_json::to_vec(&spec()).unwrap()
    );
    for host in [
        "api.example.test",
        "api.example.test:3130",
        "localhost:3130",
    ] {
        let mut value = old.clone();
        value["healthHost"] = serde_json::json!(host);
        let runtime: RuntimeSpec =
            serde_json::from_value(value).expect("optional canonical probe Host");
        runtime.validate().unwrap();
        assert_eq!(serde_json::to_value(runtime).unwrap()["healthHost"], host);
    }
    for host in [
        "",
        "API.example.test",
        "api.example.test/path",
        "https://api.example.test",
        "user@api.example.test",
        "api.example.test\r\nX-Test: true",
        "api.example.test:0",
        "api.example.test:65536",
        "api.example.test:03130",
        "api.example.test.",
        "*.example.test",
        "-api.example.test",
    ] {
        let mut value = old.clone();
        value["healthHost"] = serde_json::json!(host);
        assert!(
            serde_json::from_value::<RuntimeSpec>(value)
                .map_or(true, |runtime| runtime.validate().is_err()),
            "{host:?}"
        );
    }
}
fn journal_directory() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
fn observed() -> ObservedState {
    ObservedState {
        autoscaling: None,
        resource_id: "456".into(),
        revision: 1,
        phase: Phase::Running,
        ready: true,
        observed_at: 1,
        runtime_id: Some("runtime-1".into()),
        conditions: vec![],
        storage: None,
    }
}
#[test]
fn rejects_unpinned_privileged_or_unbounded_workloads() {
    assert!(spec().validate().is_ok());
    for bad in [
        RuntimeSpec {
            image: "busybox:latest".into(),
            ..spec()
        },
        RuntimeSpec {
            cpu_millicores: 0,
            ..spec()
        },
        RuntimeSpec {
            memory_bytes: 1,
            ..spec()
        },
        RuntimeSpec { port: 22, ..spec() },
        RuntimeSpec {
            installation: "../../host".into(),
            ..spec()
        },
        RuntimeSpec {
            volume: Some(RetainedVolume {
                mount_path: "/var/run/docker.sock".into(),
                size_bytes: None,
            }),
            ..spec()
        },
    ] {
        assert!(bad.validate().is_err());
    }
    let mut value = serde_json::to_value(spec()).unwrap();
    value["privileged"] = true.into();
    assert!(serde_json::from_value::<RuntimeSpec>(value).is_err());
}
#[test]
fn journal_commits_receipts_fences_generations_and_recovers_interrupted_work() {
    let directory = journal_directory();
    let path = directory.path().join("journal.json");
    let c = command(1);
    {
        let mut journal = Journal::open(&path).unwrap();
        assert!(matches!(
            journal.claim(&c).unwrap(),
            Claim::Execute { recovery: false }
        ));
    }
    {
        let mut journal = Journal::open(&path).unwrap();
        assert!(matches!(
            journal.claim(&c).unwrap(),
            Claim::Execute { recovery: true }
        ));
        journal.complete(&c, &observed()).unwrap();
    }
    let mut journal = Journal::open(&path).unwrap();
    assert!(matches!(journal.claim(&c).unwrap(), Claim::Replay(_)));
    let mut changed = c.clone();
    changed.spec.arguments.push("different".into());
    assert!(journal.claim(&changed).is_err());
    assert!(matches!(
        journal.claim(&command(2)).unwrap(),
        Claim::Execute { .. }
    ));
    assert!(matches!(journal.claim(&c), Err(ExecutionError::Fenced)));
    assert!(Journal::open(&path).is_err());
}
struct Never;
#[async_trait::async_trait]
impl RuntimeAdapter for Never {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    async fn reconcile(
        &self,
        _: &Command,
        _: &LeaseGuard,
    ) -> Result<ObservedState, ExecutionError> {
        panic!("denied command reached runtime")
    }
    async fn inspect(&self, _: &RuntimeSpec) -> Result<ObservedState, ExecutionError> {
        panic!("not used")
    }
    async fn logs(&self, _: &RuntimeSpec, _: u32) -> Result<String, ExecutionError> {
        Err(ExecutionError::Unsupported("logs"))
    }
}
#[tokio::test]
async fn executor_denies_missing_authority_and_expired_leases_before_mutation() {
    let directory = journal_directory();
    let executor = Executor::new(
        Journal::open(directory.path().join("journal.json")).unwrap(),
        Arc::new(DenyAll),
        Arc::new(Never),
    );
    assert!(matches!(
        executor.execute(command(1)).await,
        Err(ExecutionError::Denied)
    ));
    let mut expired = command(1);
    expired.lease_until = 0;
    assert!(executor.execute(expired).await.is_err());
}

struct Grant(std::sync::atomic::AtomicBool);
#[async_trait::async_trait]
impl Authorizer for Grant {
    async fn authorize(&self, _: &Command) -> Result<()> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ExecutionError::Denied)
        }
    }
}
struct Pending {
    entered: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl RuntimeAdapter for Pending {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    async fn reconcile(&self, _: &Command, _: &LeaseGuard) -> Result<ObservedState> {
        self.entered.notify_one();
        std::future::pending().await
    }
    async fn inspect(&self, _: &RuntimeSpec) -> Result<ObservedState> {
        Ok(observed())
    }
    async fn logs(&self, _: &RuntimeSpec, _: u32) -> Result<String> {
        Ok(String::new())
    }
}
#[tokio::test]
async fn revocation_during_work_persists_reconciliation_and_never_success() {
    let directory = journal_directory();
    let path = directory.path().join("journal.json");
    let authority = Arc::new(Grant(std::sync::atomic::AtomicBool::new(true)));
    let pending = Arc::new(Pending {
        entered: tokio::sync::Notify::new(),
    });
    let executor = Arc::new(Executor::new(
        Journal::open(&path).unwrap(),
        authority.clone(),
        pending.clone(),
    ));
    let task = {
        let executor = executor.clone();
        tokio::spawn(async move { executor.execute(command(1)).await })
    };
    pending.entered.notified().await;
    authority
        .0
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(task.await.unwrap(), Err(ExecutionError::Denied)));
    drop(executor);
    let mut journal = Journal::open(&path).unwrap();
    assert!(matches!(
        journal.claim(&command(1)).unwrap(),
        Claim::Execute { recovery: true }
    ));
}
#[test]
fn journal_rejects_symlink_state_and_same_generation_competing_operation() {
    let directory = journal_directory();
    let path = directory.path().join("journal.json");
    std::os::unix::fs::symlink("/etc/passwd", &path).unwrap();
    assert!(Journal::open(&path).is_err());
    std::fs::remove_file(&path).unwrap();
    let mut journal = Journal::open(&path).unwrap();
    journal.claim(&command(1)).unwrap();
    let mut competing = command(1);
    competing.operation_id = "another-operation".into();
    assert!(matches!(
        journal.claim(&competing),
        Err(ExecutionError::Fenced)
    ));
}

#[tokio::test]
async fn lease_cannot_be_reused_for_a_different_resource_or_operation() {
    let allowed = command(1);
    let guard = LeaseGuard::new(
        allowed.clone(),
        Arc::new(Grant(std::sync::atomic::AtomicBool::new(true))),
    );
    assert!(guard.check_for(&allowed).await.is_ok());
    let mut other = allowed.clone();
    other.spec.resource_id = RecordId::parse("789").unwrap();
    assert!(guard.check_for(&other).await.is_err());
    let mut other = allowed;
    other.operation_id = "different-operation".into();
    assert!(guard.check_for(&other).await.is_err());
}

#[test]
fn resource_revisions_and_per_operation_lease_generations_are_separate_fences() {
    let directory = journal_directory();
    let mut journal = Journal::open(directory.path().join("journal.json")).unwrap();
    let first = command(4);
    journal.claim(&first).unwrap();
    journal.complete(&first, &observed()).unwrap();
    let mut next = command(1);
    next.operation_id = "next-revision".into();
    next.spec.revision = 2;
    assert!(matches!(
        journal.claim(&next).unwrap(),
        Claim::Execute { recovery: false }
    ));
    assert!(matches!(journal.claim(&first), Err(ExecutionError::Fenced)));
}
#[test]
fn old_journal_replays_exact_generation_but_cannot_infer_renewed_payload_identity() {
    let directory = journal_directory();
    let path = directory.path().join("journal.json");
    let first = command(1);
    let result = observed();
    {
        let mut journal = Journal::open(&path).unwrap();
        journal.claim(&first).unwrap();
        journal.complete(&first, &result).unwrap();
    }
    let mut old: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let receipt = old["commands"][&first.operation_id]
        .as_object_mut()
        .unwrap();
    receipt.remove("payload_fingerprint");
    receipt.remove("generation");
    old["resources"][first.spec.key()]
        .as_object_mut()
        .unwrap()
        .remove("revision");
    let bytes = serde_json::to_vec(&old).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    {
        let mut journal = Journal::open(&path).unwrap();
        match journal.claim(&first).unwrap() {
            Claim::Replay(actual) => assert_eq!(actual, result),
            _ => panic!("original completed generation must remain replayable"),
        }
        let mut renewed = first.clone();
        renewed.generation += 1;
        assert!(matches!(
            journal.claim(&renewed),
            Err(ExecutionError::Conflict)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    assert!(matches!(
        Journal::open(&path).unwrap().claim(&first).unwrap(),
        Claim::Replay(actual) if actual == result
    ));
}

#[test]
fn renewed_generation_requires_verification_of_completed_payload_but_never_changed_input() {
    let directory = journal_directory();
    let mut journal = Journal::open(directory.path().join("journal.json")).unwrap();
    let first = command(1);
    journal.claim(&first).unwrap();
    journal.complete(&first, &observed()).unwrap();
    let mut renewed = first.clone();
    renewed.generation = 2;
    assert!(matches!(journal.claim(&renewed).unwrap(), Claim::Verify(_)));
    renewed.generation = 3;
    renewed.spec.arguments.push("changed".into());
    assert!(matches!(
        journal.claim(&renewed),
        Err(ExecutionError::Conflict)
    ));
}

#[test]
fn retirement_and_purge_have_distinct_authorized_fingerprints_and_storage_proofs() {
    let mut c = command(1);
    c.action = Action::Retire;
    let retire = c.fingerprint().unwrap();
    c.action = Action::PurgeStorage;
    assert_ne!(retire, c.fingerprint().unwrap());
    let mut retired = observed();
    retired.phase = Phase::Absent;
    retired.ready = true;
    retired.runtime_id = None;
    retired.storage = Some(StorageObservation {
        volume_bytes: Some(67108864),
        retained: true,
    });
    let value = serde_json::to_value(&retired).unwrap();
    assert_eq!(value["storage"]["volumeBytes"], 67108864);
    assert_eq!(value["storage"]["retained"], true);
    retired.storage = Some(StorageObservation {
        volume_bytes: Some(0),
        retained: false,
    });
    assert_eq!(
        serde_json::to_value(retired).unwrap()["storage"]["volumeBytes"],
        0
    );
}

#[test]
fn retained_image_and_argument_limits_match_utf16_contract() {
    let mut c = command(1);
    let prefix = "registry.example/";
    let suffix = format!("@sha256:{}", "a".repeat(64));
    c.spec.image = format!(
        "{}{}{}",
        prefix,
        "a".repeat(512 - prefix.len() - suffix.len()),
        suffix
    );
    c.spec.arguments = vec!["é".repeat(512), "🦀".repeat(256)];
    assert!(c.validate().is_ok());
    c.spec.arguments.push("🦀".repeat(257));
    assert!(c.validate().is_err());
    c.spec.arguments.clear();
    c.spec.image.insert(prefix.len(), 'a');
    assert!(c.validate().is_err());
}

#[test]
fn hosted_environment_material_is_scope_bound_and_never_serialized_with_commands() {
    let mut c = command(1);
    c.spec.hosted_access = Some(HostedAccess {
        service_bindings: None,
        environment: Some(EnvironmentReference {
            revision: c.spec.revision,
            source_sha256: "b".repeat(64),
        }),
    });
    let request = c.fingerprint().unwrap();
    assert!(c.verify_material().is_err());
    c.material = Some(RuntimeMaterial {
        service_bindings: None,
        operation_id: c.operation_id.clone(),
        request_sha256: request,
        environment: Some(EnvironmentMaterial {
            source_sha256: "b".repeat(64),
            values: std::collections::BTreeMap::from([(
                "TOKEN".into(),
                "fixture-sensitive-value".into(),
            )]),
        }),
    });
    c.verify_material().unwrap();
    let serialized = serde_json::to_string(&c).unwrap();
    assert!(!serialized.contains("fixture-sensitive-value"));
    assert!(!serialized.contains("\"material\""));
    let decoded: Command = serde_json::from_str(&serialized).unwrap();
    assert!(decoded.material.is_none());
    assert_eq!(decoded.fingerprint().unwrap(), c.fingerprint().unwrap());
    c.material.as_mut().unwrap().operation_id = "another-operation".into();
    assert!(c.verify_material().is_err());
}

#[test]
fn service_material_binds_both_revisions_and_fixed_destinations() {
    let preserve = HostedAccess {
        environment: None,
        service_bindings: None,
    };
    let clear = HostedAccess {
        environment: None,
        service_bindings: Some(vec![]),
    };
    assert!(
        serde_json::to_value(&preserve)
            .unwrap()
            .get("serviceBindings")
            .is_none()
    );
    assert_eq!(
        serde_json::to_value(&clear).unwrap()["serviceBindings"],
        serde_json::json!([])
    );

    let reference:ServiceBindingReference=serde_json::from_value(serde_json::json!({"kind":"mail","id":"123","revision":2,"credentialRevision":7,"egressCIDRs":["203.0.113.7/32"]})).unwrap();
    reference.validate().unwrap();
    let bad:ServiceBindingReference=serde_json::from_value(serde_json::json!({"kind":"mail","id":"123","revision":2,"credentialRevision":7,"egressCIDRs":["127.0.0.1/32"]})).unwrap();
    assert!(bad.validate().is_err());
    let broad:ServiceBindingReference=serde_json::from_value(serde_json::json!({"kind":"mail","id":"123","revision":2,"credentialRevision":7,"egressCIDRs":["203.0.113.0/24"]})).unwrap();
    assert!(broad.validate().is_err());
    let material: ServiceBindingMaterial = serde_json::from_value(
        serde_json::json!({"kind":"mail","id":"123","revision":2,"credentialRevision":7}),
    )
    .unwrap();
    reference.verify(&material).unwrap();
    let changed: ServiceBindingMaterial = serde_json::from_value(
        serde_json::json!({"kind":"mail","id":"123","revision":2,"credentialRevision":8}),
    )
    .unwrap();
    assert!(reference.verify(&changed).is_err());
}

struct InspectionRuntime {
    state: std::sync::Mutex<ObservedState>,
    applies: std::sync::atomic::AtomicUsize,
    inspections: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl RuntimeAdapter for InspectionRuntime {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    async fn reconcile(&self, command: &Command, guard: &LeaseGuard) -> Result<ObservedState> {
        guard.check_for(command).await?;
        self.applies
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut state = self.state.lock().unwrap();
        state.observed_at = timestamp()?;
        Ok(state.clone())
    }
    async fn inspect(&self, _: &RuntimeSpec) -> Result<ObservedState> {
        self.inspections
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut state = self.state.lock().unwrap();
        state.observed_at = timestamp()?;
        Ok(state.clone())
    }
    async fn logs(&self, _: &RuntimeSpec, _: u32) -> Result<String> {
        Err(ExecutionError::Unsupported("logs"))
    }
}
#[tokio::test]
async fn renewed_completed_generation_reinspects_without_reapplying_or_retiming_old_evidence() {
    use std::sync::atomic::Ordering::SeqCst;
    let directory = journal_directory();
    let runtime = Arc::new(InspectionRuntime {
        state: std::sync::Mutex::new(observed()),
        applies: 0.into(),
        inspections: 0.into(),
    });
    let executor = Executor::new(
        Journal::open(directory.path().join("journal.json")).unwrap(),
        Arc::new(Grant(true.into())),
        runtime.clone(),
    );
    let first = command(1);
    let completed = executor.execute(first.clone()).await.unwrap();
    assert_eq!(executor.execute(first.clone()).await.unwrap(), completed);
    assert_eq!(runtime.applies.load(SeqCst), 1);
    assert_eq!(runtime.inspections.load(SeqCst), 0);
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let mut renewed = first.clone();
    renewed.generation = 2;
    renewed.lease_until = timestamp().unwrap() + 300;
    let verified = executor.execute(renewed.clone()).await.unwrap();
    assert!(
        verified.observed_at > completed.observed_at,
        "new lease must carry a new physical observation"
    );
    assert_eq!(
        runtime.applies.load(SeqCst),
        1,
        "renewal must not blindly reapply a completed mutation"
    );
    assert_eq!(runtime.inspections.load(SeqCst), 1);
    assert_eq!(executor.execute(renewed.clone()).await.unwrap(), verified);
    assert_eq!(runtime.inspections.load(SeqCst), 1);
    assert!(matches!(
        executor.execute(first).await,
        Err(ExecutionError::Fenced)
    ));
    let mut changed = renewed.clone();
    changed.generation = 3;
    changed.spec.arguments.push("different".into());
    assert!(matches!(
        executor.execute(changed).await,
        Err(ExecutionError::Conflict)
    ));
    runtime.state.lock().unwrap().runtime_id = Some("replaced-runtime".into());
    renewed.generation = 3;
    assert!(matches!(
        executor.execute(renewed.clone()).await,
        Err(ExecutionError::Ownership)
    ));
    assert!(matches!(
        executor.execute(renewed.clone()).await,
        Err(ExecutionError::Ownership)
    ));
    assert_eq!(runtime.applies.load(SeqCst), 1);
    drop(executor);
    let executor = Executor::new(
        Journal::open(directory.path().join("journal.json")).unwrap(),
        Arc::new(Grant(true.into())),
        runtime.clone(),
    );
    assert!(
        matches!(
            executor.execute(renewed.clone()).await,
            Err(ExecutionError::Ownership)
        ),
        "restart must retain verification-only custody"
    );
    {
        let mut state = runtime.state.lock().unwrap();
        state.runtime_id = Some("runtime-1".into());
        state.ready = false;
    }
    assert!(
        matches!(
            executor.execute(renewed.clone()).await,
            Err(ExecutionError::Runtime)
        ),
        "owned but unconverged state cannot become success"
    );
    assert_eq!(runtime.applies.load(SeqCst), 1);
    renewed.generation = 4;
    renewed.lease_until = timestamp().unwrap() - 1;
    assert!(matches!(
        executor.execute(renewed).await,
        Err(ExecutionError::Expired)
    ));
}
