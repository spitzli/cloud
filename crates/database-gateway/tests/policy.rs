use serde_json::json;
use webdock_database_gateway::policy::{Profile, authorize_rpc, public_result};
#[test]
fn bound_connection_replaces_only_identity_fields_and_blocks_host_configuration() {
    for command in [
        "save_connection",
        "save_config",
        "install_plugin",
        "dump_database",
        "run_ssh_command",
        "new_upstream_command",
    ] {
        assert!(authorize_rpc(command, json!({}), "123", "internal", Profile::Read).is_err());
    }
    assert!(
        authorize_rpc(
            "execute_query",
            json!({"connectionId":"another","query":"select 1"}),
            "123",
            "internal",
            Profile::Read
        )
        .is_err()
    );
    assert_eq!(
        authorize_rpc(
            "execute_query",
            json!({"connectionId":"123","query":"select '123'"}),
            "123",
            "internal",
            Profile::Read
        )
        .unwrap(),
        json!({"connectionId":"internal","query":"select '123'"})
    );
    assert!(
        authorize_rpc(
            "insert_record",
            json!({"connectionId":"123"}),
            "123",
            "internal",
            Profile::Read
        )
        .is_err()
    );
    assert!(
        authorize_rpc(
            "drop_view",
            json!({"connectionId":"123"}),
            "123",
            "internal",
            Profile::Write
        )
        .is_err()
    );
}
#[test]
fn browser_results_hide_saved_credentials_and_restore_local_scope() {
    let value = json!([{"id":"internal","name":"Orders","params":{"driver":"postgres","database":"orders","host":"private","username":"owner","password":"secret"}},{"id":"other","params":{}}]);
    assert_eq!(
        public_result("get_connections", value, "123", "internal", Profile::Read).unwrap(),
        json!([{"id":"123","name":"Orders","params":{"driver":"postgres","database":"orders"}}])
    );
    let value = public_result(
        "get_driver_manifest",
        json!({"capabilities":{"readonly":false,"manage_tables":true,"user_management":true}}),
        "123",
        "internal",
        Profile::Read,
    )
    .unwrap();
    assert_eq!(value["capabilities"]["readonly"], true);
    assert_eq!(value["capabilities"]["manage_tables"], false);
    assert_eq!(value["capabilities"]["user_management"], false);
    let input = json!({"connectionId":"123","preferences":{"tabs":[{"id":"tab","connectionId":"123","query":"select '123'"}]}});
    let saved = authorize_rpc(
        "save_editor_preferences",
        input,
        "123",
        "internal",
        Profile::Read,
    )
    .unwrap();
    let restored = public_result(
        "load_editor_preferences",
        saved["preferences"].clone(),
        "456",
        "internal",
        Profile::Read,
    )
    .unwrap();
    assert_eq!(restored["tabs"][0]["connectionId"], "456");
    assert_eq!(restored["tabs"][0]["query"], "select '123'");
}
