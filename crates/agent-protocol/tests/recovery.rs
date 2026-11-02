use webdock_agent_protocol::recovery::*;
#[test]
fn recovery_is_explicit_bounded_fresh_and_has_no_arbitrary_external_dependency_profile() {
    let mut spec:RecoverySpec=serde_json::from_value(serde_json::json!({"version":1,"operationID":"123","policyID":"124","policyRevision":1,"clusterID":"125","profile":"ntfy-sqlite-offline-v1","action":"backup","source":{"appID":"126"},"target":null,"recoverySetID":null,"epoch":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","capacityBytes":67108864})).unwrap();
    spec.validate().unwrap();
    let hash = spec.fingerprint().unwrap();
    assert_eq!(hash.len(), 64);
    spec.target = Some(serde_json::json!({"appID":"127"}));
    assert!(spec.validate().is_err());
    spec.action = RecoveryAction::Restore;
    spec.recovery_set_id = Some("128".into());
    spec.validate().unwrap();
    spec.capacity_bytes = 1;
    assert!(spec.validate().is_err());
    assert!(
        serde_json::from_value::<RecoveryProfile>(serde_json::json!("arbitrary-volume")).is_err()
    );
}
