use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use spitzli_repository_protocol::{
    Action, LeaseClaims,
    connection::{ConnectionAction, ConnectionClaims, ConnectionScope, verify_connection},
    verify_descriptor, verify_lease,
};

const NOW: i64 = 10_000;

fn claims() -> ConnectionClaims {
    ConnectionClaims {
        v: 1,
        purpose: "repository-connection".into(),
        audience: "webdock.repository.connection.runtime.v1".into(),
        scope: ConnectionScope {
            flow_id: uuid::Uuid::new_v4().to_string(),
            operation_id: uuid::Uuid::new_v4().to_string(),
            customer_id: "11".into(),
            subject: "12".into(),
            session_id: "browser-session-fixture".into(),
            plugin_id: "fixture-repository".into(),
            generation: 1,
            action: ConnectionAction::Begin,
            descriptor_sha256: "a".repeat(64),
            request_sha256: "b".repeat(64),
            configuration_sha256: "c".repeat(64),
        },
        iat: NOW,
        exp: NOW + 60,
    }
}

fn signed_json(value: &Value, key: &SigningKey, domain: &[u8]) -> String {
    let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap());
    let mut message = domain.to_vec();
    message.extend_from_slice(body.as_bytes());
    format!(
        "{body}.{}",
        URL_SAFE_NO_PAD.encode(key.sign(&message).to_bytes())
    )
}

#[test]
fn each_connection_action_round_trips_with_exact_scope() {
    let key = SigningKey::from_bytes(&[60; 32]);
    for action in [
        ConnectionAction::Begin,
        ConnectionAction::Exchange,
        ConnectionAction::Options,
        ConnectionAction::Finish,
        ConnectionAction::Abort,
    ] {
        let mut c = claims();
        c.scope.action = action;
        let token = c.sign(&key).unwrap();
        assert_eq!(
            verify_connection(&token, &key.verifying_key(), &c.scope, NOW).unwrap(),
            c
        );
        assert!(
            verify_connection(
                &token,
                &SigningKey::from_bytes(&[61; 32]).verifying_key(),
                &c.scope,
                NOW
            )
            .is_err()
        );
    }
}

#[test]
fn changing_any_expected_binding_denies_the_token() {
    let key = SigningKey::from_bytes(&[60; 32]);
    let c = claims();
    let token = c.sign(&key).unwrap();
    for (field, replacement) in [
        ("flowID", json!(uuid::Uuid::new_v4().to_string())),
        ("operationID", json!(uuid::Uuid::new_v4().to_string())),
        ("customerID", json!("21")),
        ("subject", json!("22")),
        ("sessionID", json!("another-browser-session")),
        ("pluginID", json!("other-repository")),
        ("generation", json!(2)),
        ("action", json!("exchange")),
        ("descriptorSha256", json!("d".repeat(64))),
        ("requestSha256", json!("e".repeat(64))),
        ("configurationSha256", json!("f".repeat(64))),
    ] {
        let mut expected = serde_json::to_value(&c.scope).unwrap();
        expected[field] = replacement;
        let expected = serde_json::from_value(expected).unwrap();
        assert!(
            verify_connection(&token, &key.verifying_key(), &expected, NOW).is_err(),
            "{field}"
        );
    }
}

#[test]
fn connection_authority_cannot_be_reused_as_source_or_session_authority() {
    let key = SigningKey::from_bytes(&[60; 32]);
    let c = claims();
    let token = c.sign(&key).unwrap();
    assert!(verify_lease(&token, &key.verifying_key(), &c.scope.plugin_id, 1, NOW).is_err());
    assert!(verify_descriptor(&token, &key.verifying_key()).is_err());
    assert!(
        spitzli_plugin_protocol::verify_invocation(
            &token,
            &key.verifying_key(),
            &c.scope.plugin_id,
            1,
            NOW
        )
        .is_err()
    );
    let source = LeaseClaims {
        v: 1,
        purpose: "repository".into(),
        audience: "webdock.repository.runtime.v1".into(),
        id: uuid::Uuid::new_v4().to_string(),
        plugin_id: c.scope.plugin_id.clone(),
        generation: 1,
        action: Action::Archive,
        customer_id: "11".into(),
        project_id: "13".into(),
        source_id: "14".into(),
        source_revision: 1,
        connection_generation: 1,
        grant_revision: 1,
        configuration_revision: 0,
        descriptor_sha256: "a".repeat(64),
        request_sha256: "b".repeat(64),
        configuration_sha256: "c".repeat(64),
        iat: NOW,
        exp: NOW + 60,
    }
    .sign(&key)
    .unwrap();
    assert!(verify_connection(&source, &key.verifying_key(), &c.scope, NOW).is_err());
    let wrong_domain = signed_json(
        &serde_json::to_value(&c).unwrap(),
        &key,
        b"webdock.repository.lease.v1\0",
    );
    assert!(verify_connection(&wrong_domain, &key.verifying_key(), &c.scope, NOW).is_err());
}

#[test]
fn malformed_claims_fail_even_with_a_valid_issuer_signature() {
    let key = SigningKey::from_bytes(&[60; 32]);
    let c = claims();
    for (pointer, replacement) in [
        ("/v", json!(2)),
        ("/purpose", json!("repository")),
        ("/audience", json!("webdock.repository.runtime.v1")),
        ("/scope/flowID", json!("not-a-uuid")),
        ("/scope/operationID", json!(uuid::Uuid::nil().to_string())),
        ("/scope/customerID", json!("01")),
        ("/scope/subject", json!("0")),
        ("/scope/sessionID", json!("")),
        ("/scope/sessionID", json!("x".repeat(257))),
        ("/scope/sessionID", json!("line\nbreak")),
        ("/scope/sessionID", json!("has space")),
        ("/scope/pluginID", json!("Bad.Plugin")),
        ("/scope/generation", json!(0)),
        ("/scope/generation", json!(9_007_199_254_740_992_u64)),
        ("/scope/descriptorSha256", json!("A".repeat(64))),
        ("/scope/requestSha256", json!("b".repeat(63))),
        ("/scope/configurationSha256", json!("g".repeat(64))),
        ("/scope/action", json!("archive")),
        ("/iat", json!(NOW + 1)),
        ("/exp", json!(NOW)),
        ("/exp", json!(NOW + 61)),
        ("/iat", json!(i64::MIN)),
    ] {
        let mut value = serde_json::to_value(&c).unwrap();
        *value.pointer_mut(pointer).unwrap() = replacement;
        let token = signed_json(&value, &key, b"webdock.repository.connection.v1\0");
        assert!(
            verify_connection(&token, &key.verifying_key(), &c.scope, NOW).is_err(),
            "{pointer}"
        );
        if let Ok(parsed) = serde_json::from_value::<ConnectionClaims>(value)
            && parsed.iat <= NOW
        {
            assert!(parsed.sign(&key).is_err(), "sign: {pointer}");
        }
    }
    for pointer in ["", "/scope"] {
        let mut value = serde_json::to_value(&c).unwrap();
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("projectID".into(), json!("13"));
        assert!(
            verify_connection(
                &signed_json(&value, &key, b"webdock.repository.connection.v1\0"),
                &key.verifying_key(),
                &c.scope,
                NOW
            )
            .is_err()
        );
    }
}

#[test]
fn time_and_token_boundaries_are_checked_without_panics() {
    let key = SigningKey::from_bytes(&[60; 32]);
    let mut c = claims();
    c.scope.session_id = "s".repeat(256);
    c.scope.generation = 9_007_199_254_740_991;
    c.exp = NOW + 1;
    let token = c.sign(&key).unwrap();
    assert!(verify_connection(&token, &key.verifying_key(), &c.scope, NOW).is_ok());
    assert!(verify_connection(&token, &key.verifying_key(), &c.scope, NOW - 1).is_err());
    assert!(verify_connection(&token, &key.verifying_key(), &c.scope, NOW + 1).is_err());
    for invalid in [
        "".to_owned(),
        "x.y.z".to_owned(),
        format!("{token}x"),
        "x".repeat(16385),
    ] {
        assert!(verify_connection(&invalid, &key.verifying_key(), &c.scope, NOW).is_err());
    }
    c.iat = i64::MIN;
    c.exp = i64::MAX;
    assert!(c.sign(&key).is_err());
}
