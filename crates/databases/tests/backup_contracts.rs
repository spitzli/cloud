use serde_json::json;
use webdock_databases::ManagedCommand;
#[test]
fn operator_policy_commands_and_exact_recovery_are_native_contracts() {
    for action in [
        "databases.backupPolicies.grant",
        "databases.backupPolicies.revoke",
        "databases.backupPolicies.get",
    ] {
        let mut v = json!({"action":action,"databaseID":"101"});
        if action != "databases.backupPolicies.get" {
            v["idempotencyKey"] = json!("backup-policy-fixture-001");
            v["revision"] = json!(1);
        }
        if action == "databases.backupPolicies.revoke" {
            v["policyRevision"] = json!(1);
        }
        if action == "databases.backupPolicies.grant" {
            v["destinationID"] = json!("102");
            v["schedule"] = json!("0 0 1 * * *");
            v["retentionDays"] = json!(30);
            v["retentionDelete"] = json!(false);
        }
        assert!(ManagedCommand::parse(v).is_ok(), "{action}");
    }
    let restore = json!({"action":"databases.restore","sourceDatabaseID":"101","restorePointID":"102","targetProjectID":"103","targetOrderID":"104","revision":1,"credentialRevision":1,"targetTime":"2026-10-25T12:00:00Z","timeline":1,"verificationSHA256":"a".repeat(64),"name":"Recovered","idempotencyKey":"restore-fixture-001"});
    assert!(ManagedCommand::parse(restore.clone()).is_ok());
    let mut bad = restore;
    bad.as_object_mut().unwrap().remove("restorePointID");
    assert!(ManagedCommand::parse(bad).is_err());
}
#[test]
fn restore_requires_expected_data_hash_and_never_selects_latest() {
    let v = json!({"action":"databases.restore","sourceDatabaseID":"101","restorePointID":"102","targetProjectID":"103","targetOrderID":"104","revision":1,"credentialRevision":1,"name":"Recovered","targetTime":"2026-10-25T12:00:00Z","timeline":1,"verificationSHA256":"a".repeat(64),"idempotencyKey":"restore-fixture-001"});
    assert!(ManagedCommand::parse(v.clone()).is_ok());
    let mut bad = v;
    bad["timeline"] = json!("latest");
    assert!(ManagedCommand::parse(bad).is_err());
}
#[test]
fn public_backup_status_keeps_unqualified_actions_disabled_and_observations_separate() {
    let schema: serde_json::Value =
        serde_json::from_str(webdock_databases::BACKUP_STATUS_SCHEMA).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let absent = json!({"status":"not-configured","lastSuccessAt":null,"recoverableUntil":null,"lastRestoreTestAt":null,"observedControls":null,"publicRecoveryAvailable":false,"disabledReason":"production-qualification-required","rpoSeconds":null,"rtoSeconds":null});
    assert!(validator.is_valid(&absent));
    let mut bad = absent;
    bad["publicRecoveryAvailable"] = json!(true);
    assert!(!validator.is_valid(&bad));
}
