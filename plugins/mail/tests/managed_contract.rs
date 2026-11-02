use serde_json::json;
use webdock_mail::{
    delivery::{private_policy_digest, public_reply},
    executor::DeliveryConfiguration,
    transport::Platform,
};
#[test]
fn private_platform_policy_pins_effective_configuration_and_replies_strip_authority() {
    let a = Platform::Smtp {
        host: "127.0.0.1".into(),
        port: 123,
    };
    let b = Platform::Smtp {
        host: "127.0.0.1".into(),
        port: 124,
    };
    assert_ne!(
        private_policy_digest("101", None, &a),
        private_policy_digest("101", None, &b)
    );
    let relay = Some(uuid::Uuid::new_v4());
    assert_eq!(
        private_policy_digest("101", relay, &a),
        private_policy_digest("101", relay, &b)
    );
    assert_ne!(
        private_policy_digest("101", relay, &a),
        private_policy_digest("102", relay, &a)
    );
    assert_eq!(
        public_reply(&json!({"id":"123","state":"unknown","leaseToken":"private"})).unwrap(),
        json!({"id":"123","state":"unknown"})
    );
    for bad in [
        json!({"id":"01","state":"queued"}),
        json!({"id":"123","state":"sent"}),
    ] {
        assert!(public_reply(&bad).is_err());
    }
}
#[test]
fn strict_delivery_config_has_explicit_native_issuer_and_account_selection() {
    let binding = json!({"pluginID":"mail","generation":2,"descriptorSha256":"a".repeat(64),"workerID":"mail","workerGeneration":2,"workerCredentialSha256":"b".repeat(64)});
    let config = json!({"nativeLeaseIssuerPublicKeyHex":"c".repeat(64),"providerBinding":binding,"gatewayPeers":[],"nativeAccounts":[{"customerID":"101","accountID":null,"mode":"managed","transportRevision":1,"bindingRevision":2,"byokRevision":null,"transport":{"kind":"smtp","host":"127.0.0.1","port":123}}]});
    assert!(serde_json::from_value::<DeliveryConfiguration>(config.clone()).is_ok());
    for field in [
        "nativeLeaseIssuerPublicKeyHex",
        "gatewayPeers",
        "nativeAccounts",
    ] {
        let mut missing = config.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<DeliveryConfiguration>(missing).is_err());
    }
    for field in ["accountID", "byokRevision"] {
        let mut missing = config.clone();
        missing["nativeAccounts"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(serde_json::from_value::<DeliveryConfiguration>(missing).is_err());
    }
    let mut extra = config;
    extra["fallbackProjectID"] = json!("123");
    assert!(serde_json::from_value::<DeliveryConfiguration>(extra).is_err());
}

#[test]
fn binding_derives_only_normal_mtls_and_validates_explicit_enrollment() {
    use webdock_mail::executor::resolve_provider_binding;
    let raw = b"exact raw manifest bytes";
    let derived =
        resolve_provider_binding(None, 2, raw, Some(b"own leaf"), Some(b"own leaf")).unwrap();
    assert_eq!(
        derived.descriptor_sha256,
        webdock_mail_protocol::sha256(raw)
    );
    assert_eq!(
        derived.worker_credential_sha256,
        webdock_mail_protocol::sha256(b"own leaf")
    );
    assert_eq!(derived.worker_id, "mail");
    assert!(resolve_provider_binding(None, 2, raw, None, None).is_err());
    assert!(resolve_provider_binding(None, 2, raw, Some(b"listen"), Some(b"callback")).is_err());
    let mut wrong = derived.clone();
    wrong.descriptor_sha256 = "f".repeat(64);
    assert!(resolve_provider_binding(Some(wrong), 2, raw, None, None).is_err());
    let mut manual = derived;
    manual.worker_credential_sha256 = "e".repeat(64);
    assert!(resolve_provider_binding(Some(manual), 2, raw, None, None).is_ok());
}
