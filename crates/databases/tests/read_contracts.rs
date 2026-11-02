use serde_json::json;
use webdock_databases::ManagedCommand;

#[test]
fn safe_operation_read_uses_strict_managed_contract() {
    let input = json!({"action":"databases.operations.get","databaseID":"101","operationID":"202"});
    assert!(ManagedCommand::parse(input.clone()).unwrap().is_read());
    for field in [
        "authority",
        "desired",
        "nativeAuthority",
        "lease",
        "providerCredentials",
    ] {
        let mut bad = input.clone();
        bad[field] = json!({});
        assert!(ManagedCommand::parse(bad).is_err());
    }
}

#[test]
fn read_boundary_rejects_writes_secrets_authority_and_unbounded_queries() {
    use webdock_databases::DatabaseReadCommand;
    for input in [
        json!({"action":"databases.offers.create","projectID":"101","planRevisionID":"202","name":"Stale new database","idempotencyKey":"read-must-not-quote"}),
        json!({"action":"databases.acceptOrder","projectID":"101","offerID":"202","idempotencyKey":"read-must-not-accept"}),
        json!({"action":"databases.backup","databaseID":"101","revision":1,"idempotencyKey":"read-must-not-write"}),
        json!({"action":"databases.credentials","databaseID":"101"}),
        json!({"action":"databases.restore","databaseID":"101"}),
        json!({"action":"databases.get","databaseID":"101","authority":null}),
        json!({"action":"databases.list","projectID":"101","limit":101}),
        json!({"action":"databases.list","projectID":"101","limit":0}),
        json!({"action":"databases.list","projectID":"101","cursor":"-1"}),
        json!({"action":"databases.list","projectID":"101","cursor":"9223372036854775808"}),
        json!({"action":"databases.get","databaseID":"101","cursor":"2"}),
        json!({"action":"databases.installations.list","clusterID":"101"}),
    ] {
        assert!(DatabaseReadCommand::parse(input).is_err());
    }
    for action in [
        "databases.projects.list",
        "databases.list",
        "databases.plans.list",
        "databases.backupPolicies.get",
    ] {
        let mut input = match action {
            "databases.projects.list" => json!({"customerID":"101"}),
            "databases.list" | "databases.plans.list" => json!({"projectID":"101"}),
            _ => json!({"databaseID":"101"}),
        };
        input["action"] = json!(action);
        input["limit"] = json!(2);
        input["cursor"] = json!("102");
        DatabaseReadCommand::parse(input).unwrap();
    }
}

#[test]
fn discovery_is_exact_read_allowlist_with_strict_native_schemas() {
    let tools = webdock_databases::database_read_tools();
    assert_eq!(tools.len(), 7);
    for tool in tools {
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        let schema = &tool["inputSchema"];
        assert_eq!(schema["additionalProperties"], false);
        for forbidden in [
            "action",
            "authority",
            "nativeAuthority",
            "writeCredentials",
            "recoveryCredentials",
            "token",
        ] {
            assert!(schema["properties"].get(forbidden).is_none());
        }
    }
}
