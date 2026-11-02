use serde_json::json;
use webdock_mail::managed_control::{Command, EXECUTOR, STORE_ID};
#[test]
fn action_control_rejects_redundant_authority() {
    let valid = json!({"action":"send.admitAction","submission":{"invocationToken":"opaque","input":{},"payloadRef":uuid::Uuid::new_v4().to_string(),"privatePolicySha256":"a".repeat(64)}});
    assert!(serde_json::from_value::<Command>(valid.clone()).is_ok());
    for field in [
        "customerID",
        "operationID",
        "envelope",
        "requestHash",
        "payloadBytes",
        "workerID",
    ] {
        let mut bad = valid.clone();
        bad["submission"][field] = json!("caller");
        assert!(serde_json::from_value::<Command>(bad).is_err());
    }
    assert!(
        serde_json::from_value::<Command>(
            json!({"action":"send.status","token":"opaque","id":"123","cancel":true})
        )
        .is_ok()
    );
    assert_eq!(EXECUTOR, "managed-mail-v1");
    assert_eq!(STORE_ID, "mail:platform:v1");
}
