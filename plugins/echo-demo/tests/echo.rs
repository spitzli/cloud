use serde_json::json;
use webdock_echo_demo::{Echo, validate_manifest};
use webdock_plugin_protocol::{InvocationClaims, Manifest};
use webdock_plugin_runtime::ActionHandler;

fn claims(action: &str) -> InvocationClaims {
    serde_json::from_value(json!({
        "v":1,"id":"00000000-0000-4000-8000-000000000001","pluginID":"echo-demo","generation":1,
        "action":action,"projectID":"201","customerID":"101","iat":0,"exp":60
    }))
    .unwrap()
}
async fn call(
    action: &str,
    input: serde_json::Value,
) -> Result<serde_json::Value, webdock_plugin_protocol::PluginError> {
    Echo.invoke(&claims(action), "t", input, json!({})).await
}
#[tokio::test]
async fn echoes_within_the_signed_project_and_rejects_everything_else() {
    assert_eq!(
        call("demo.echo", json!({"message":"hi"})).await.unwrap(),
        json!({"message":"hi","projectID":"201"})
    );
    for (action, input, configuration) in [
        ("demo.echo", json!({"message":"x".repeat(201)}), json!({})),
        ("demo.echo", json!({"message":"a\nb"}), json!({})),
        ("demo.echo", json!({"other":true}), json!({})),
        ("demo.echo", json!({}), json!({"x":1})),
        ("demo.unknown", json!({}), json!({})),
    ] {
        assert!(
            Echo.invoke(&claims(action), "t", input, configuration)
                .await
                .is_err()
        );
    }
}
#[tokio::test]
async fn notes_create_view_reveal_and_delete() {
    let created = call("notes.create", json!({"name":"Shopping"}))
        .await
        .unwrap();
    let id = created["resource"]["id"].as_str().unwrap().to_owned();
    assert_eq!(created["resource"]["name"], "Shopping");
    assert!(uuid::Uuid::parse_str(&id).is_ok());
    assert_ne!(
        call("notes.create", json!({"name":"Shopping"}))
            .await
            .unwrap()["resource"]["id"],
        id.as_str()
    );
    let panel = call("notes.view", json!({"resource_id":id})).await.unwrap();
    assert_eq!(panel["version"], 2);
    let kinds: Vec<&str> = panel["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["status", "reveal", "actions"]);
    assert_eq!(panel["blocks"][1]["action"], "notes.secret");
    assert_eq!(panel["blocks"][2]["actions"][0]["id"], "notes.delete");
    assert!(
        !panel.to_string().contains(&format!("echo:{id}")),
        "the secret is not panel data"
    );
    assert_eq!(
        call("notes.secret", json!({"resource_id":id}))
            .await
            .unwrap(),
        json!({"value": format!("echo:{id}")})
    );
    assert_eq!(
        call("notes.delete", json!({"resource_id":id}))
            .await
            .unwrap(),
        json!({})
    );
    assert_eq!(
        call("demo.operator", json!({})).await.unwrap()["version"],
        2
    );
    for (action, input) in [
        ("notes.create", json!({"name":""})),
        ("notes.create", json!({"name":"a\u{7}"})),
        ("notes.create", json!({"name":"x".repeat(81)})),
        ("notes.view", json!({})),
        ("notes.view", json!({"resource_id":"a/b"})),
    ] {
        assert!(
            call(action, input.clone()).await.is_err(),
            "{action} {input}"
        );
    }
}
#[test]
fn only_the_reviewed_demo_manifest_is_accepted() {
    let raw = include_str!("../manifest.json");
    validate_manifest(&Manifest::parse(raw.as_bytes()).unwrap()).unwrap();
    let mut changed: serde_json::Value = serde_json::from_str(raw).unwrap();
    changed["capabilities"] = json!(["plugin.http.read"]);
    let changed = serde_json::to_vec(&changed).unwrap();
    assert!(Manifest::parse(&changed).map_or(true, |m| validate_manifest(&m).is_err()));
}
