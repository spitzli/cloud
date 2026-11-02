//! Serialization-only private authority tests; no credential fixture or authentication claim.
use serde_json::{Value, json};
use webdock_browser_session::BrowserAuthority;
use webdock_browser_session::StoredAuthority;
#[path = "../src/database_authority.rs"]
mod database_authority;
use database_authority::DatabaseAuthority;

#[test]
fn old_browser_wire_is_not_rewritten_or_reinterpreted_as_mcp() {
    for old in [
        StoredAuthority::LocalSession {},
        StoredAuthority::CustomerSession {
            customer_scope: "101".into(),
            read_only: true,
        },
        StoredAuthority::FirstPartyBrowser {
            browser: BrowserAuthority {
                reference: "serialization-fixture-reference".into(),
                client_id: "fixture-console".into(),
                app_origin: "https://console.example.test".into(),
                expires_at: 2000000000,
                role: "operator".into(),
                scopes: vec!["hosting:read".into()],
                read_only: true,
                customer_scope: Some("101".into()),
            },
        },
    ] {
        let wire = serde_json::to_value(&old).unwrap();
        let parsed = DatabaseAuthority::from_persisted(wire.clone()).unwrap();
        assert_eq!(parsed.to_persisted().unwrap(), wire);
        assert_eq!(
            serde_json::to_vec(&parsed.to_persisted().unwrap()).unwrap(),
            serde_json::to_vec(&wire).unwrap()
        );
        assert_eq!(parsed.read_only(), old.read_only());
    }
    let retained = DatabaseAuthority::from_persisted(Value::Null).unwrap();
    assert_eq!(
        retained.to_persisted().unwrap(),
        json!({"kind":"local-session"})
    );
}

#[test]
fn malformed_mcp_reference_never_falls_back_to_local_browser_authority() {
    for value in [
        json!({"kind":"database-mcp","version":1,"reference":{}}),
        json!({"kind":"database-mcp","version":0,"reference":null}),
        json!({"kind":"mcp","reference":{}}),
        json!({"kind":"local-session","reference":{"kind":"database-mcp"}}),
        json!({"kind":"database-mcp","version":1,"reference":null,"browser":{"kind":"local-session"}}),
    ] {
        assert!(DatabaseAuthority::from_persisted(value).is_err());
    }
}

#[test]
fn public_commands_cannot_supply_any_private_authority_variant() {
    for field in [
        "authority",
        "nativeAuthority",
        "mcpAuthority",
        "subject",
        "sessionID",
    ] {
        let mut command = json!({"action":"databases.get","databaseID":"101"});
        command[field] = json!({"kind":"database-mcp"});
        assert!(webdock_databases::ManagedCommand::parse(command).is_err());
    }
}

#[test]
fn strict_private_mcp_codec_roundtrip_is_not_authentication_or_browser_fallback() {
    // Untrusted codec candidate only: no principal, guard, issuer/token row or authentication success.
    let value = json!({"kind":"database-mcp","version":1,"reference":{
        "v":1,"accessId":"untrusted-codec-only","tokenHash":"a".repeat(43),"recordHash":"b".repeat(43),
        "callerId":"untrusted-caller","issuerClientId":"untrusted-client","subject":"untrusted-subject","sessionId":"untrusted-session",
        "resource":"https://studio.example.test/api/mcp","requiredScope":"hosting:write","write":true,"role":"hosting-operator","issuedAt":1900000000,"expiresAt":1900000300
    }});
    let parsed = DatabaseAuthority::from_persisted(value.clone()).unwrap();
    assert!(matches!(&parsed, DatabaseAuthority::Mcp(_)));
    assert_eq!(parsed.to_persisted().unwrap(), value);
    for field in value["reference"].as_object().unwrap().keys() {
        let mut missing = value.clone();
        missing["reference"].as_object_mut().unwrap().remove(field);
        assert!(DatabaseAuthority::from_persisted(missing).is_err());
    }
    let mut extra = value.clone();
    extra["reference"]["browser"] = json!({"kind":"local-session"});
    assert!(DatabaseAuthority::from_persisted(extra).is_err());
    let mut wrong = value;
    wrong["reference"]["expiresAt"] = json!("1900000300");
    assert!(DatabaseAuthority::from_persisted(wrong).is_err());
    // expires_at() is deliberately NOT called on this unverified candidate.
}

#[test]
fn manual_backup_commands_cannot_submit_retained_authority_or_claim_durable_policy() {
    let input = json!({"action":"databases.backup","databaseID":"101","revision":1,"idempotencyKey":"original-proof-contract-123"});
    webdock_databases::ManagedCommand::parse(input.clone()).unwrap();
    for field in [
        "authority",
        "nativeAuthority",
        "mcpAuthority",
        "policyAuthority",
        "backupPolicyID",
        "actor_session",
    ] {
        let mut bad = input.clone();
        bad[field] = json!({"kind":"database-mcp"});
        assert!(webdock_databases::ManagedCommand::parse(bad).is_err());
    }
}
