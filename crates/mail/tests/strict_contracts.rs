use serde_json::json;
use webdock_mail::{budget::SendPath, outbox::Receipt};

#[test]
fn direct_path_preserves_its_wire_shape_and_rejects_unknown_fields() {
    let wire = json!({"kind": "direct"});
    let path: SendPath = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(path).unwrap(), wire);
    assert!(
        serde_json::from_value::<SendPath>(
            json!({"kind": "direct", "serviceRevision": "untrusted"})
        )
        .is_err()
    );
}

#[test]
fn definitive_and_uncertain_receipts_reject_unexpected_provider_ids() {
    for state in ["rejected", "unknown"] {
        let wire = json!({"state": state});
        let receipt: Receipt = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(receipt).unwrap(), wire);
        assert!(
            serde_json::from_value::<Receipt>(json!({"state": state, "providerID": "untrusted"}))
                .is_err()
        );
    }
}
