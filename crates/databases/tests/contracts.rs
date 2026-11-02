use serde_json::json;
use webdock_databases::{Command, normalize_origin};
#[test]
fn runtime_origin_and_grant_acknowledgments_are_strict() {
    for bad in [
        "http://remote.invalid",
        "https://a.invalid/path",
        "https://a.invalid?x=1",
        "https://u:p@a.invalid",
        "https://a.invalid/#x",
        "https://a.invalid\\evil",
    ] {
        assert!(normalize_origin(bad).is_err(), "{bad}");
    }
    assert_eq!(
        normalize_origin("https://a.invalid/").unwrap(),
        "https://a.invalid"
    );
    assert_eq!(
        normalize_origin("http://127.0.0.1:8001").unwrap(),
        "http://127.0.0.1:8001"
    );
    let mut grant = json!({"action":"grant","bindingID":"123","subject":"user-123","profile":"read","runtimeOrigin":"https://runtime.invalid","connectionID":"connection","proxySecret":"x".repeat(32),"isolationVerified":true,"databaseRoleVerified":true});
    Command::parse(grant.clone()).unwrap();
    grant["databaseRoleVerified"] = json!(false);
    assert!(Command::parse(grant.clone()).is_err());
    grant["databaseRoleVerified"] = json!(true);
    grant["accessToken"] = json!("browser bearer");
    assert!(Command::parse(grant).is_err());
}
#[test]
fn identifiers_and_names_do_not_accept_ambiguous_input() {
    for id in ["", "-1", "0", "01", "1 OR 1=1", "9223372036854775808"] {
        assert!(Command::parse(json!({"action":"get","bindingID":id})).is_err());
    }
    assert!(
        Command::parse(
            json!({"action":"create","projectID":"1","name":"  ","environment":"production"})
        )
        .is_err()
    );
}
