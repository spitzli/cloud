use ed25519_dalek::SigningKey;
use webdock_mail_protocol::*;
fn claims() -> LeaseClaims {
    LeaseClaims {
        v: 1,
        purpose: "mail-send".into(),
        audience: "webdock.mail.runtime.v1".into(),
        id: uuid::Uuid::new_v4().to_string(),
        plugin_id: "mail-fixture".into(),
        generation: 1,
        customer_id: "123".into(),
        job_id: "456".into(),
        source_id: "key:789".into(),
        transport_revision: 1,
        grant_revision: 1,
        configuration_revision: 0,
        descriptor_sha256: "a".repeat(64),
        binding_sha256: "b".repeat(64),
        worker_id: "mail-worker".into(),
        worker_generation: 1,
        worker_credential_sha256: "c".repeat(64),
        metadata_sha256: "d".repeat(64),
        mime_sha256: "e".repeat(64),
        mime_bytes: 100,
        iat: 1000,
        exp: 1060,
    }
}
#[test]
fn lease_signature_purpose_current_generation_expiry_and_bounds_are_enforced() {
    let key = SigningKey::from_bytes(&[72; 32]);
    let expected = claims();
    let signed = expected.sign(&key).unwrap();
    assert_eq!(
        verify_lease(&signed, &key.verifying_key(), "mail-fixture", 1, 1000).unwrap(),
        expected
    );
    for (plugin, generation, now) in [
        ("other-plugin", 1, 1000),
        ("mail-fixture", 2, 1000),
        ("mail-fixture", 1, 1060),
        ("mail-fixture", 1, 990),
    ] {
        assert!(verify_lease(&signed, &key.verifying_key(), plugin, generation, now).is_err());
    }
    assert!(
        verify_lease(
            &signed,
            &SigningKey::from_bytes(&[73; 32]).verifying_key(),
            "mail-fixture",
            1,
            1000
        )
        .is_err()
    );
    assert!(
        verify_lease(
            &"a".repeat(17000),
            &key.verifying_key(),
            "mail-fixture",
            1,
            1000
        )
        .is_err()
    );
    for field in [
        "purpose", "audience", "bytes", "zero", "customer", "source", "expiry", "digest",
    ] {
        let mut bad = claims();
        match field {
            "purpose" => bad.purpose = "repository".into(),
            "audience" => bad.audience = "webdock.plugin.v1".into(),
            "bytes" => bad.mime_bytes = MAX_MIME_BYTES + 1,
            "zero" => bad.mime_bytes = 0,
            "customer" => bad.customer_id = "9223372036854775808".into(),
            "source" => bad.source_id = "\n".into(),
            "expiry" => bad.exp = 1061,
            _ => bad.mime_sha256 = "X".repeat(64),
        }
        assert!(bad.sign(&key).is_err());
    }
}
#[test]
fn action_source_is_exact_persisted_operation_hash() {
    let mut c = claims();
    c.source_id = format!("action:{}", "a".repeat(64));
    assert!(c.validate(1000).is_ok());
    for source in [
        format!("action:{}", "A".repeat(64)),
        "action:123".into(),
        format!("action:{}", "a".repeat(65)),
        "action:".into(),
        "key:01".into(),
    ] {
        c.source_id = source;
        assert!(c.validate(1000).is_err());
    }
}
