use serde_json::json;
use webdock_plugin_protocol::Manifest;
#[test]
fn native_mail_service_is_explicit_and_availability_checked() {
    let mut v: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/project-notes.json")).unwrap();
    v["services"]["required"] = json!([{"name":"native-mail-control","version":1}]);
    v["services"]["optional"] = json!([]);
    let manifest = Manifest::parse(v.to_string().as_bytes()).unwrap();
    assert!(
        manifest
            .require_host_services(&[("data", 1), ("http", 1), ("jobs", 1)])
            .is_err()
    );
    assert!(
        manifest
            .require_host_services(&[("native-mail-control", 1)])
            .is_ok()
    );
}
