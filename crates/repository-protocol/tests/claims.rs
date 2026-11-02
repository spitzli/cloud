use ed25519_dalek::SigningKey;
use webdock_repository_protocol::*;
#[test]
fn repository_tokens_are_bound_and_cannot_be_session_invocations() {
    let key = SigningKey::from_bytes(&[19; 32]);
    let now = 10_000;
    let mut claims = LeaseClaims {
        v: 1,
        purpose: "repository".into(),
        audience: "webdock.repository.runtime.v1".into(),
        id: uuid::Uuid::new_v4().to_string(),
        plugin_id: "fixture-repository".into(),
        generation: 1,
        action: Action::Archive,
        customer_id: "11".into(),
        project_id: "12".into(),
        source_id: "13".into(),
        source_revision: 1,
        connection_generation: 1,
        grant_revision: 1,
        configuration_revision: 0,
        descriptor_sha256: "a".repeat(64),
        request_sha256: "b".repeat(64),
        configuration_sha256: "c".repeat(64),
        iat: now,
        exp: now + 60,
    };
    let token = claims.sign(&key).unwrap();
    assert_eq!(
        verify_lease(&token, &key.verifying_key(), "fixture-repository", 1, now).unwrap(),
        claims
    );
    assert!(verify_lease(&token, &key.verifying_key(), "fixture-repository", 2, now).is_err());
    assert!(verify_lease(&token, &key.verifying_key(), "other-repository", 1, now).is_err());
    assert!(
        verify_lease(
            &token,
            &key.verifying_key(),
            "fixture-repository",
            1,
            now + 60
        )
        .is_err()
    );
    assert!(
        webdock_plugin_protocol::verify_invocation(
            &token,
            &key.verifying_key(),
            "fixture-repository",
            1,
            now
        )
        .is_err()
    );
    claims.exp = now + 61;
    assert!(claims.sign(&key).is_err());
}
#[test]
fn descriptor_uses_distinct_signing_domain_and_exact_release_digest() {
    let key = SigningKey::from_bytes(&[20; 32]);
    let descriptor = Descriptor {
        v: 1,
        plugin_id: "fixture-repository".into(),
        version: "1.0.0".into(),
        manifest_sha256: "a".repeat(64),
        image: format!("registry.invalid/plugin@sha256:{}", "b".repeat(64)),
        protocol: "webdock.repository.v1".into(),
        signer_key_id: "fixture-key".into(),
    };
    let token = descriptor.sign(&key).unwrap();
    assert_eq!(
        verify_descriptor(&token, &key.verifying_key()).unwrap(),
        descriptor
    );
    assert!(verify_lease(&token, &key.verifying_key(), "fixture-repository", 1, 1).is_err());
    assert!(verify_descriptor(&token, &SigningKey::from_bytes(&[21; 32]).verifying_key()).is_err());
}
