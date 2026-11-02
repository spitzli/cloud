use serde_json::{Value, json};
fn validator() -> jsonschema::Validator {
    jsonschema::options()
        .should_validate_formats(true)
        .build(&serde_json::from_str(include_str!("../src/restore-order.schema.json")).unwrap())
        .unwrap()
}
fn prepare() -> Value {
    json!({"action":"databases.restoreOrder.prepare","sourceDatabaseID":"101","revision":2,"restorePointID":"102","credentialRevision":1,
        "targetTime":"2026-10-25T10:00:01+00:00","timeline":1,"verificationSHA256":"a".repeat(64),"targetProjectID":"103",
        "targetPlanRevisionID":"104","name":"Recovered","idempotencyKey":"restore-order-prepare-001"})
}
fn accept() -> Value {
    json!({"action":"databases.restoreOrder.accept","offerID":"105","projectID":"103","intentHash":"b".repeat(64),"confirmPrice":true,
        "reviewedPrice":{"monthlyPriceCents":1200,"currency":"EUR","billing":"monthly-net-fixed","billingStarts":"observed-ready"},"idempotencyKey":"restore-order-accept-001"})
}
#[test]
fn candidate_dtos_require_every_exact_prepare_and_explicit_reviewed_price_field() {
    let validator = validator();
    for command in [prepare(), accept()] {
        assert!(validator.is_valid(&command));
        for field in command.as_object().unwrap().keys() {
            let mut missing = command.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(!validator.is_valid(&missing), "{field}");
        }
    }
}
#[test]
fn candidate_dtos_reject_injected_authority_secrets_unknown_purpose_and_false_price_consent() {
    let validator = validator();
    for command in [prepare(), accept()] {
        for field in [
            "authority",
            "nativeAuthority",
            "mcpAuthority",
            "subject",
            "sourceCnpgUID",
            "password",
            "writeCredentials",
            "restoreIntent",
        ] {
            let mut injected = command.clone();
            injected[field] = json!("fixture-only");
            assert!(!validator.is_valid(&injected));
        }
    }
    for (field, value) in [
        ("confirmPrice", json!(false)),
        ("intentHash", json!("latest")),
        ("purpose", json!("create")),
    ] {
        let mut invalid = accept();
        invalid[field] = value;
        assert!(!validator.is_valid(&invalid));
    }
}
#[test]
fn live_parser_remains_unwired_and_browser_backup_contracts_remain_unchanged() {
    assert!(webdock_databases::ManagedCommand::parse(prepare()).is_err());
    assert!(webdock_databases::ManagedCommand::parse(accept()).is_err());
    webdock_databases::ManagedCommand::parse(
        json!({"action":"databases.backupPolicies.get","databaseID":"101"}),
    )
    .unwrap();
}
