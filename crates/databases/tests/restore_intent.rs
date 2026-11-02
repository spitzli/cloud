//! Pure, unlinked Part B contract. No service/worker/SQL/agent admission wiring.
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use webdock_databases::ManagedCommand;

#[path = "../src/restore_intent.rs"]
mod restore_intent;
use restore_intent::{RestoreAcceptance, RestoreIntent, require_ordinary_offer};

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-25T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}
fn review() -> Value {
    let recorded: Value = serde_json::from_str(include_str!(
        "../../../docs/verification/2026-10-25-native-postgresql-backup-transitions.json"
    ))
    .unwrap();
    let policy = recorded["connectedEvidence"]["resize"]["policy"]["spec"].clone();
    json!({
        "version":1,"purpose":"restore","offerID":"2001","expiresAt":"2026-10-25T12:30:00Z",
        "source":{"databaseID":"2002","revision":2,"clusterID":"2003","policy":policy,
            "restorePointID":"2004","baseBackupID":"20261025T100000","completedAt":"2026-10-25T10:00:00Z","credentialRevision":1},
        "target":{"customerID":policy["customerID"],"projectID":policy["projectID"],"clusterID":"2003","planRevisionID":"2005","name":"Recovered",
            "plan":{"kind":"dedicated","postgresMajor":18,"name":"Dedicated restore","monthlyPriceCents":1200,"currency":"EUR",
                "cpuMillicores":1000,"memoryBytes":2147483648_u64,"volumeBytes":10737418240_u64,"logicalStorageAllowanceBytes":null,"connectionLimit":10,
                "terms":"Reviewed immutable fixed monthly net database price. Billing starts only after observed readiness."}},
        "targetTime":"2026-10-25T10:00:01Z","timeline":1,"verificationSHA256":"a".repeat(64)
    })
}
fn acceptance(intent: &RestoreIntent) -> Value {
    json!({"action":"databases.restoreOrder.accept","offerID":"2001","projectID":review()["target"]["projectID"],
        "intentHash":intent.hash().unwrap(),"confirmPrice":true,
        "reviewedPrice":{"monthlyPriceCents":1200,"currency":"EUR","billing":"monthly-net-fixed","billingStarts":"observed-ready"},
        "idempotencyKey":"restore-intent-accept-001"})
}

#[test]
fn reviewed_intent_accepts_historical_credentials_and_full_exact_price() {
    let intent = RestoreIntent::parse(review(), now()).unwrap();
    let accept = RestoreAcceptance::parse(acceptance(&intent)).unwrap();
    intent.validate_acceptance(&accept, now()).unwrap();
    assert_eq!(intent.source.credential_revision, 1);
    assert_eq!(intent.hash().unwrap().len(), 64);
}

#[test]
fn canonical_hash_is_object_order_independent_but_binds_all_recovery_and_price_inputs() {
    let value = review();
    let intent = RestoreIntent::parse(value.clone(), now()).unwrap();
    let ordered = Value::Object(
        value
            .as_object()
            .unwrap()
            .iter()
            .rev()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    assert_eq!(
        intent.hash().unwrap(),
        RestoreIntent::parse(ordered, now())
            .unwrap()
            .hash()
            .unwrap()
    );
    for pointer in [
        "/offerID",
        "/expiresAt",
        "/source/databaseID",
        "/source/revision",
        "/source/clusterID",
        "/source/policy/installationID",
        "/source/policy/sourceCnpgUID",
        "/source/policy/revision",
        "/source/policy/resourceRevision",
        "/source/policy/destination/endpointURL",
        "/source/policy/destination/archivePath",
        "/source/policy/destination/serverName",
        "/source/policy/destination/credentialRevision",
        "/source/policy/destination/recoveryCredentialRevision",
        "/source/policy/destination/keyRecoveryID",
        "/source/restorePointID",
        "/source/baseBackupID",
        "/source/completedAt",
        "/source/credentialRevision",
        "/target/customerID",
        "/target/projectID",
        "/target/clusterID",
        "/target/planRevisionID",
        "/target/name",
        "/target/plan/monthlyPriceCents",
        "/target/plan/currency",
        "/target/plan/terms",
        "/targetTime",
        "/timeline",
        "/verificationSHA256",
    ] {
        let mut changed = value.clone();
        let field = changed.pointer_mut(pointer).unwrap();
        *field = if field.is_number() {
            json!(field.as_i64().unwrap() + 1)
        } else {
            json!(format!("{}-changed", field.as_str().unwrap()))
        };
        let bound = RestoreIntent::parse(changed, now()).map(|v| v.hash().unwrap());
        assert!(
            bound.is_err() || bound.unwrap() != intent.hash().unwrap(),
            "{pointer}"
        );
    }
}

#[test]
fn accept_rejects_changed_price_hash_missing_consent_and_foreign_scope() {
    let intent = RestoreIntent::parse(review(), now()).unwrap();
    for (pointer, replacement) in [
        ("/confirmPrice", json!(false)),
        ("/reviewedPrice/monthlyPriceCents", json!(0)),
        ("/reviewedPrice/currency", json!("USD")),
        ("/reviewedPrice/billingStarts", json!("accepted")),
        ("/intentHash", json!("b".repeat(64))),
        ("/projectID", json!("9999")),
        ("/offerID", json!("9999")),
    ] {
        let mut value = acceptance(&intent);
        *value.pointer_mut(pointer).unwrap() = replacement;
        let parsed = RestoreAcceptance::parse(value);
        assert!(
            parsed.is_err() || intent.validate_acceptance(&parsed.unwrap(), now()).is_err(),
            "{pointer}"
        );
    }
    let mut missing = acceptance(&intent);
    missing.as_object_mut().unwrap().remove("confirmPrice");
    assert!(RestoreAcceptance::parse(missing).is_err());
    assert!(
        intent
            .validate_acceptance(
                &RestoreAcceptance::parse(acceptance(&intent)).unwrap(),
                now() + chrono::Duration::minutes(30)
            )
            .is_err()
    );
}

#[test]
fn intent_rejects_cross_project_shared_future_or_before_base_and_secret_injection() {
    for (pointer, replacement) in [
        ("/target/projectID", json!("9999")),
        ("/target/customerID", json!("9999")),
        ("/target/clusterID", json!("9999")),
        ("/target/plan/kind", json!("shared")),
        ("/targetTime", json!("2026-10-25T09:59:59Z")),
        ("/targetTime", json!("2026-10-25T12:00:01Z")),
        ("/source/credentialRevision", json!(0)),
        ("/timeline", json!(0)),
        ("/verificationSHA256", json!("latest")),
    ] {
        let mut value = review();
        *value.pointer_mut(pointer).unwrap() = replacement;
        assert!(RestoreIntent::parse(value, now()).is_err(), "{pointer}");
    }
    for secret in [
        "authority",
        "password",
        "writeCredentials",
        "nativeAuthority",
    ] {
        let mut value = review();
        value[secret] = json!("fixture-only-forbidden");
        assert!(RestoreIntent::parse(value, now()).is_err());
    }
}

#[test]
fn ordinary_offer_checks_reject_restore_purpose_even_when_a_consumed_order_already_exists() {
    require_ordinary_offer(&json!({"kind":"dedicated","backupStatus":"not-configured"})).unwrap();
    for value in [
        json!({"purpose":"restore","restoreIntent":review()}),
        json!({"purpose":"other"}),
        json!({"purpose":true}),
    ] {
        assert!(require_ordinary_offer(&value).is_err());
    }
}
