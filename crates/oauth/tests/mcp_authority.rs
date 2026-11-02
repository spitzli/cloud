use serde_json::{Value, json};
use webdock_oauth::McpAuthority;

// A parser fixture is not authenticated authority; production must revalidate it.
fn stored_reference() -> Value {
    json!({
        "v": 1,
        "accessId": "123",
        "tokenHash": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "recordHash": "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
        "callerId": "456",
        "issuerClientId": "789",
        "subject": "101",
        "sessionId": "102",
        "resource": "https://studio.example.invalid/api/mcp",
        "requiredScope": "webdock:read",
        "write": false,
        "role": "operator",
        "issuedAt": 1793000000_i64,
        "expiresAt": 1793000300_i64
    })
}

#[test]
fn original_expiry_is_an_exact_unix_deadline_without_wire_changes() {
    for expiry in [i64::MIN, -1, 0, 1793000300, i64::MAX] {
        let mut wire = stored_reference();
        wire["expiresAt"] = json!(expiry);
        let authority: McpAuthority = serde_json::from_value(wire.clone()).unwrap();
        let deadline: i64 = authority.expires_at();
        assert_eq!(deadline, expiry);
        assert_eq!(serde_json::to_value(&authority).unwrap(), wire);
        assert_eq!(authority.subject(), "101");
        assert_eq!(authority.session_id(), "102");
        assert!(authority.registry_scope());
        assert!(!authority.can_write());
    }
}

#[test]
fn stored_reference_parser_rejects_unexpected_or_malformed_expiry_fields() {
    let mut unknown = stored_reference();
    unknown["rawToken"] = json!("not-an-access-token");
    assert!(serde_json::from_value::<McpAuthority>(unknown).is_err());
    for expiry in [
        Value::Null,
        json!("1793000300"),
        json!(1.5),
        json!(u64::MAX),
    ] {
        let mut wire = stored_reference();
        wire["expiresAt"] = expiry;
        assert!(serde_json::from_value::<McpAuthority>(wire).is_err());
    }
    let mut missing = stored_reference();
    missing.as_object_mut().unwrap().remove("expiresAt");
    assert!(serde_json::from_value::<McpAuthority>(missing).is_err());
}
