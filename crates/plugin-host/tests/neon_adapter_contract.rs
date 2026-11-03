use serde_json::json;
use webdock_plugin_host::http::{validate_named_request, validate_request};

#[test]
fn structured_query_is_exclusive_to_named_neon_adapter() {
    let input = json!({"binding":"neon","method":"GET","path":"/projects","query":{"cursor":"AbC+/=?:&org_id=other#fragment","limit":"100"}});
    assert!(
        validate_request(&input).is_err(),
        "legacy static contract must stay strict"
    );
    assert!(validate_named_request(&input).is_ok());
    for input in [
        json!({"binding":"other","method":"GET","path":"/projects","query":{}}),
        json!({"binding":"neon","method":"GET","path":"//attacker.invalid/projects"}),
        json!({"binding":"neon","method":"GET","path":"/projects?org_id=other"}),
        json!({"binding":"neon","method":"GET","path":"/projects","query":{"org_id":"other"}}),
        json!({"binding":"neon","method":"GET","path":"/projects","headers":{"authorization":"secret"}}),
        json!({"binding":"neon","method":"GET","path":"/projects","query":{"cursor":["a"]}}),
    ] {
        assert!(validate_named_request(&input).is_err());
    }
}
