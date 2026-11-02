//! Controlled admission cases for the genuine source factory. No captured-set assertion.
//! Link only once the real daemon has advertised an operator-installed recovery profile,
//! then stop that owned daemon before this driver claims work (no competing executor).
use super::*;
use webdock_agent_protocol::{self as wire, recovery::RecoveryClaim};
use wire::recovery_control_client::RecoveryControlClient;

pub(super) struct PolicyBatch<'a> {
    pub app: &'a Router,
    pub console: &'a str,
    pub owner: &'a DatabaseConnection,
    pub cluster: &'a str,
    pub source: &'a str,
    pub profile_id: &'a str,
    /// Actual negotiated identity from this source's enrolled, stopped daemon.
    pub identity: wire::AgentIdentity,
    pub control_path: &'a Path,
}
fn grant(c: &PolicyBatch<'_>, key: &str, capacity: u64) -> Value {
    json!({"action":"recovery.policies.grant","clusterID":c.cluster,"profile":"ntfy-sqlite-offline-v1",
        "sourceAppID":c.source,"profileId":c.profile_id,"capacityBytes":capacity,"intervalSeconds":60,"idempotencyKey":key})
}
fn backup(policy: &str, revision: u64, key: &str) -> Value {
    json!({"action":"recovery.backup","policyID":policy,"revision":revision,"idempotencyKey":key})
}
fn revoke(policy: &str, revision: u64, key: &str) -> Value {
    json!({"action":"recovery.policies.revoke","policyID":policy,"revision":revision,"idempotencyKey":key})
}
async fn conflict(reply: Reply, stage: &str) {
    assert_eq!(
        reply.status,
        StatusCode::CONFLICT,
        "{stage}: {}",
        draft_policy_denial_detail(reply.status, &reply.body)
    );
}
async fn counts(c: &PolicyBatch<'_>) -> Value {
    row(c.owner,"SELECT jsonb_build_object('policies',(SELECT count(*) FROM webdock_auth.hosting_recovery_policy),
        'operations',(SELECT count(*) FROM webdock_auth.hosting_recovery_job),
        'reservations',(SELECT count(*) FROM webdock_auth.hosting_recovery_reservation),
        'sets',(SELECT count(*) FROM webdock_auth.hosting_recovery_set)) AS value",vec![]).await
}
fn checkpoint(
    identity: &wire::AgentIdentity,
    claim: &RecoveryClaim,
    value: Value,
) -> wire::RecoveryCheckpointRequest {
    wire::RecoveryCheckpointRequest {
        identity: Some(identity.clone()),
        claim_json: serde_json::to_vec(claim).unwrap(),
        phase: "checkpoint".into(),
        checkpoint_json: serde_json::to_vec(&value).unwrap(),
    }
}
fn denied<T>(reply: Result<tonic::Response<T>, tonic::Status>, stage: &str) {
    let error = reply
        .err()
        .unwrap_or_else(|| panic!("{stage}: denied request succeeded"));
    assert!(
        matches!(
            error.code(),
            tonic::Code::PermissionDenied
                | tonic::Code::Unauthenticated
                | tonic::Code::FailedPrecondition
        ),
        "{stage}: unexpected static status {:?}",
        error.code()
    );
}
/// Actual HTTP + enrolled Unix RPC. Discarded replies prove retained callback replay;
/// a transport lost-ACK + actual daemon outbox/restart case is still separately required.
pub(super) async fn admission_and_replay(c: PolicyBatch<'_>) {
    assert!(enabled());
    let observation=row(c.owner,"SELECT observation#>'{capabilities,nativeRecovery}' AS value FROM webdock_auth.hosting_agent WHERE cluster_id=$1 AND last_seen>clock_timestamp()-interval '90 seconds'",vec![c.cluster.into()]).await;
    assert_eq!(observation["version"], 2);
    assert_eq!(observation["appEffectFenceVersion"], 1);
    let declared = observation["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == c.profile_id)
        .unwrap();
    assert!(
        declared["storageIdentity"]["configuration"]["poolID"]
            .as_str()
            .is_some()
    );
    assert_eq!(
        declared["storageIdentity"]["configuration"]["clusterID"],
        c.cluster
    );

    // Oversized policy is a legitimate finite intent; actual Core reservation must deny
    // its complete peak atomically without creating an operation or partial reservation.
    let oversized = okay(
        post(
            c.app,
            c.console,
            grant(&c, "policy-capacity-denial", 1073741824),
        )
        .await,
        "oversized-policy",
    );
    let oversized = oversized["policyID"].as_str().unwrap();
    let before = counts(&c).await;
    conflict(
        post(c.app, c.console, backup(oversized, 1, "oversized-backup")).await,
        "complete-peak-capacity-denial",
    )
    .await;
    assert_eq!(
        counts(&c).await,
        before,
        "capacity denial committed partial state"
    );
    okay(
        post(c.app, c.console, revoke(oversized, 1, "revoke-oversized")).await,
        "revoke-oversized",
    );

    let input = grant(&c, "policy-same-key", 67108864);
    let (a, b) = tokio::join!(
        post(c.app, c.console, input.clone()),
        post(c.app, c.console, input.clone())
    );
    let a = okay(a, "same-key-grant-a");
    let b = okay(b, "same-key-grant-b");
    assert_eq!(a, b);
    let policy = a["policyID"].as_str().unwrap();
    assert_eq!(a["revision"], 1);
    // A changed request cannot use the existing receipt, nor create another live source policy.
    conflict(
        post(c.app, c.console, grant(&c, "policy-same-key", 134217728)).await,
        "changed-payload-same-key",
    )
    .await;
    conflict(
        post(
            c.app,
            c.console,
            grant(&c, "policy-different-key", 67108864),
        )
        .await,
        "different-live-policy-key",
    )
    .await;
    let before = counts(&c).await;
    conflict(
        post(c.app, c.console, backup(policy, 2, "stale-policy-revision")).await,
        "stale-policy-revision",
    )
    .await;
    assert_eq!(counts(&c).await, before);

    let a = backup(policy, 1, "backup-race-a");
    let b = backup(policy, 1, "backup-race-b");
    let (a, b) = tokio::join!(post(c.app, c.console, a), post(c.app, c.console, b));
    let (accepted, denied_reply, key) = if a.status == StatusCode::OK {
        (a, b, "backup-race-a")
    } else {
        (b, a, "backup-race-b")
    };
    let accepted = okay(accepted, "single-backup-admission");
    conflict(denied_reply, "different-key-pending-operation").await;
    assert_eq!(
        okay(
            post(c.app, c.console, backup(policy, 1, key)).await,
            "same-key-backup-replay"
        ),
        accepted
    );
    let operation = accepted["operationID"].as_str().unwrap();
    let reservation=row(c.owner,"SELECT jsonb_build_object('count',count(*),'bytes',min(peak_bytes),'poolBound',bool_and(pool_identity#>>'{configuration,clusterID}'=$2),
        'state',min(state)) AS value FROM webdock_auth.hosting_recovery_reservation WHERE operation_id=$1",vec![operation.into(),c.cluster.into()]).await;
    assert_eq!(reservation["count"], 1);
    assert_eq!(reservation["bytes"], 268435456u64);
    assert_eq!(reservation["poolBound"], true);
    assert_eq!(reservation["state"], "reserved");
    eprintln!("recovery_component policy_admission_replay_capacity=true");

    let channel = webdock_plugin_runtime::unix_channel(c.control_path)
        .await
        .unwrap();
    let mut client = RecoveryControlClient::new(channel);
    let dispatch = client
        .poll_recovery(wire::PollRequest {
            identity: Some(c.identity.clone()),
        })
        .await
        .unwrap()
        .into_inner();
    let claim: RecoveryClaim = serde_json::from_slice(&dispatch.claim_json).unwrap();
    assert_eq!(claim.spec.operation_id, operation);
    assert_eq!(claim.spec.policy_id, policy);
    assert_eq!(claim.spec.source["appID"], c.source);
    let guard = wire::RecoveryGuardRequest {
        identity: Some(c.identity.clone()),
        claim_json: dispatch.claim_json.clone(),
    };
    assert!(
        client
            .guard_recovery(guard.clone())
            .await
            .unwrap()
            .into_inner()
            .allowed
    );
    let mut stale = guard.clone();
    stale.identity.as_mut().unwrap().generation += 1;
    denied(
        client.guard_recovery(stale).await,
        "stale-enrollment-generation",
    );
    let mut foreign = guard.clone();
    foreign
        .identity
        .as_mut()
        .unwrap()
        .agent_id
        .push_str("-foreign");
    denied(
        client.guard_recovery(foreign).await,
        "foreign-enrolled-agent",
    );
    let mut changed = claim.clone();
    changed.spec.source["runtimeUID"] = json!("wrong-current-source-uid");
    denied(
        client
            .guard_recovery(wire::RecoveryGuardRequest {
                identity: Some(c.identity.clone()),
                claim_json: serde_json::to_vec(&changed).unwrap(),
            })
            .await,
        "changed-claim-source",
    );

    let request = checkpoint(&c.identity, &claim, json!({"phase":"preflight"}));
    let committed = client
        .checkpoint_recovery(request.clone())
        .await
        .unwrap()
        .into_inner();
    assert!(committed.accepted);
    let plan: Value = serde_json::from_slice(&committed.plan_json).unwrap();
    assert_eq!(plan["reservation"]["authority"]["origin"], "core");
    assert_eq!(plan["sourceFence"]["sourceKind"], "app");
    assert!(
        plan["sourceFence"]["binding"].is_null(),
        "preflight must not fabricate a stop/capture fence"
    );
    // Drop a committed result and reconnect; replay must ACK identical stored bytes.
    drop(committed);
    drop(client);
    let channel = webdock_plugin_runtime::unix_channel(c.control_path)
        .await
        .unwrap();
    let mut client = RecoveryControlClient::new(channel);
    let replay = client
        .checkpoint_recovery(request.clone())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        serde_json::from_slice::<Value>(&replay.plan_json).unwrap(),
        plan
    );
    let mut changed = request.clone();
    changed.checkpoint_json =
        serde_json::to_vec(&json!({"phase":"preflight","changed":true})).unwrap();
    denied(
        client.checkpoint_recovery(changed).await,
        "changed-checkpoint-same-phase",
    );
    okay(
        post(c.app, c.console, revoke(policy, 1, "revoke-current-policy")).await,
        "revoke-current-policy",
    );
    denied(client.guard_recovery(guard).await, "revoked-policy-guard");
    // Exact old receipt ACK cannot grant another phase or revive dispatch.
    assert!(
        client
            .checkpoint_recovery(request)
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    denied(
        client
            .checkpoint_recovery(checkpoint(
                &c.identity,
                &claim,
                json!({"phase":"after-revocation"}),
            ))
            .await,
        "revoked-new-checkpoint",
    );
    let terminal=row(c.owner,"SELECT jsonb_build_object('status',o.status,'generation',o.generation,'leaseCleared',o.lease_until IS NULL,
        'reservationRetained',r.state<>'released','sets',(SELECT count(*) FROM webdock_auth.hosting_recovery_set s WHERE s.operation_id=o.id)) AS value
        FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_recovery_reservation r ON r.operation_id=o.id WHERE o.id=$1",vec![operation.into()]).await;
    assert_eq!(terminal["status"], "needs-reconciliation");
    assert_eq!(terminal["generation"], claim.lease.generation + 1);
    assert_eq!(terminal["leaseCleared"], true);
    assert_eq!(terminal["reservationRetained"], true);
    assert_eq!(terminal["sets"], 0);
    eprintln!(
        "recovery_component enrolled_checkpoint_replay_revocation=true no_capture_claim=true"
    );
}
