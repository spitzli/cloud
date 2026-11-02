//! Pure future worker rules only. No runtime caller until single early epoch/whole-attempt integration.
//! These projections neither verify nor mint authority and cannot acknowledge external success.
use super::*;
#[derive(Debug, PartialEq, Eq)]
enum FailureDisposition {
    RetireUnissued,
    NeedsReconciliation,
    RollbackWholeAttempt,
}
#[derive(Debug, PartialEq, Eq)]
enum CallbackDisposition {
    PerformCurrentStep,
    ReceiptOnly,
    NeedsReconciliation,
}
#[derive(Clone, Copy)]
enum Request {
    Replay,
    ExplicitReconcile,
}
fn lease_limit(
    now: DateTime<Utc>,
    normal: DateTime<Utc>,
    original: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    cap_verified_lifetime(now, normal, original)
}
fn unissued(op: &Value) -> bool {
    op["status"] == "queued"
        && op["generation"] == 0
        && op["lease_until"].is_null()
        && op["lease_token_hash"].is_null()
        && op["checkpoint"].is_null()
        && op["desired"].as_object().is_some()
        && op["desired"]["databaseVersion"] == 1
        && [
            "nativeIssuedSpecs",
            "nativeDatabaseCheckpoints",
            "nativeDatabaseCompletions",
            "nativeDatabaseRetiredDeliveries",
        ]
        .iter()
        .all(|key| {
            let value = &op["desired"][key];
            value.is_null() || value.as_array().is_some_and(Vec::is_empty)
        })
}
fn failure_disposition(op: &Value, error: &Error) -> FailureDisposition {
    if error.status >= 500 || error.status == 429 {
        return FailureDisposition::RollbackWholeAttempt;
    }
    if [403, 404].contains(&error.status) && unissued(op) {
        FailureDisposition::RetireUnissued
    } else {
        FailureDisposition::NeedsReconciliation
    }
}
fn callback_disposition(
    now: DateTime<Utc>,
    until: DateTime<Utc>,
    current_authority: bool,
    recorded: bool,
) -> CallbackDisposition {
    if recorded {
        CallbackDisposition::ReceiptOnly
    } else if current_authority && until > now {
        CallbackDisposition::PerformCurrentStep
    } else {
        CallbackDisposition::NeedsReconciliation
    }
}
// A pure generation decision, not proof: runtime must retain actual fresh authorization
// and all current worker binding locks before using any of these projected facts.
fn reconcile_generation(
    op: &Value,
    expected: &Value,
    observed: &Value,
    request: Request,
) -> Option<i32> {
    if !matches!(request, Request::ExplicitReconcile)
        || op["status"] != "needs-reconciliation"
        || expected != observed
    {
        return None;
    }
    for (projected, original) in [
        ("operationID", "id"),
        ("databaseID", "database_id"),
        ("backendID", "backend_id"),
    ] {
        if expected[projected].as_str().is_none_or(str::is_empty)
            || expected[projected] != op[original]
        {
            return None;
        }
    }
    if expected["revision"].as_i64().is_none_or(|n| n < 1)
        || expected["revision"] != op["database_revision"]
        || expected["operationGeneration"] != op["generation"]
        || expected["cnpgUID"].as_str().is_none_or(str::is_empty)
        || expected["cnpgUID"] != op["checkpoint"]["cnpgUID"]
        || !expected["checkpoint"].is_object()
        || expected["checkpoint"] != op["checkpoint"]
    {
        return None;
    }
    if op["database_action"] == "restore"
        && (expected["restoreIntentHash"]
            .as_str()
            .is_none_or(|h| h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()))
            || expected["restoreIntentHash"] != op["desired"]["restoreIntentHash"])
    {
        return None;
    }
    i32::try_from(op["generation"].as_i64()?)
        .ok()?
        .checked_add(1)
}
fn queued() -> Value {
    json!({"status":"queued","generation":0,"lease_until":null,"lease_token_hash":null,"checkpoint":null,"desired":{"databaseVersion":1,"nativeIssuedSpecs":[]}})
}
fn time(value: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(value, 0).unwrap()
}
#[test]
fn original_expiry_caps_every_future_lease_and_cannot_be_renewed_by_replay() {
    assert_eq!(
        lease_limit(time(100), time(400), time(123)).unwrap(),
        time(123)
    );
    assert_eq!(
        lease_limit(time(100), time(120), time(400)).unwrap(),
        time(120)
    );
    for expiry in [99, 100] {
        assert!(lease_limit(time(100), time(400), time(expiry)).is_err());
    }
    assert!(lease_limit(time(100), time(100), time(400)).is_err());
}
#[test]
fn unissued_auth_loss_is_distinct_from_transient_or_any_issued_evidence() {
    let lost = denied();
    let transient = unavailable();
    assert_eq!(
        failure_disposition(&queued(), &lost),
        FailureDisposition::RetireUnissued
    );
    for status in [429, 500, 503] {
        assert_eq!(
            failure_disposition(
                &queued(),
                &Error {
                    status,
                    message: "isolated status rule"
                }
            ),
            FailureDisposition::RollbackWholeAttempt
        );
    }
    assert_eq!(
        failure_disposition(&queued(), &transient),
        FailureDisposition::RollbackWholeAttempt
    );
    for (key, value) in [
        ("status", json!("running")),
        ("generation", json!(1)),
        ("lease_until", json!("2030-01-01T00:00:00Z")),
        ("lease_token_hash", json!("known-issued")),
        ("checkpoint", json!({})),
    ] {
        let mut issued = queued();
        issued[key] = value;
        assert_eq!(
            failure_disposition(&issued, &lost),
            FailureDisposition::NeedsReconciliation
        );
    }
    for field in [
        "nativeIssuedSpecs",
        "nativeDatabaseCheckpoints",
        "nativeDatabaseCompletions",
        "nativeDatabaseRetiredDeliveries",
    ] {
        let mut issued = queued();
        issued["desired"][field] = json!([{}]);
        assert_eq!(
            failure_disposition(&issued, &lost),
            FailureDisposition::NeedsReconciliation
        );
        issued["desired"][field] = json!("malformed");
        assert_eq!(
            failure_disposition(&issued, &lost),
            FailureDisposition::NeedsReconciliation
        );
    }
}
#[test]
fn expired_or_revoked_original_cannot_checkpoint_promote_or_renew_but_exact_receipts_are_custody() {
    assert_eq!(
        callback_disposition(time(100), time(100), true, false),
        CallbackDisposition::NeedsReconciliation
    );
    assert_eq!(
        callback_disposition(time(100), time(200), false, false),
        CallbackDisposition::NeedsReconciliation
    );
    assert_eq!(
        callback_disposition(time(100), time(99), false, true),
        CallbackDisposition::ReceiptOnly
    );
    assert_eq!(
        callback_disposition(time(100), time(200), true, false),
        CallbackDisposition::PerformCurrentStep
    );
}
#[test]
fn only_explicit_fresh_reconcile_advances_generation_for_exact_bound_target_and_intent() {
    let op = json!({"id":"101","database_id":"102","backend_id":"103","database_revision":2,"status":"needs-reconciliation","generation":2,"database_action":"restore","checkpoint":{"phase":"applied","cnpgUID":"owned-projection"},"desired":{"nativeAuthority":{"kind":"local-session"},"restoreIntentHash":"a".repeat(64)}});
    let binding = json!({"operationID":"101","databaseID":"102","backendID":"103","revision":2,"operationGeneration":2,"cnpgUID":"owned-projection","checkpoint":op["checkpoint"],"restoreIntentHash":"a".repeat(64)});
    let original = op.clone();
    assert_eq!(
        reconcile_generation(&op, &binding, &binding, Request::ExplicitReconcile),
        Some(3)
    );
    assert_eq!(
        reconcile_generation(&op, &binding, &binding, Request::Replay),
        None
    );
    for state in ["queued", "running", "succeeded", "failed"] {
        let mut wrong = op.clone();
        wrong["status"] = json!(state);
        assert_eq!(
            reconcile_generation(&wrong, &binding, &binding, Request::ExplicitReconcile),
            None
        );
    }
    for field in [
        "operationID",
        "databaseID",
        "backendID",
        "revision",
        "operationGeneration",
        "cnpgUID",
        "checkpoint",
        "restoreIntentHash",
    ] {
        let mut wrong = binding.clone();
        wrong[field] = Value::Null;
        assert_eq!(
            reconcile_generation(&op, &binding, &wrong, Request::ExplicitReconcile),
            None
        );
    }
    let mut stale = binding.clone();
    stale["restoreIntentHash"] = json!("b".repeat(64));
    assert_eq!(
        reconcile_generation(&op, &binding, &stale, Request::ExplicitReconcile),
        None
    );
    assert_eq!(
        op, original,
        "pure generation selection never replaces original authority/target/results"
    );
}
