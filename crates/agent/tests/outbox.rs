use std::os::unix::fs::PermissionsExt;
use webdock_agent::outbox::ReceiptOutbox;
use webdock_agent_protocol::{Outcome, Receipt};
#[test]
fn exact_receipt_survives_restart_until_acknowledged_and_cannot_be_overwritten() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = dir.path().join("receipts.json");
    let receipt = Receipt {
        operation_id: "operation-1".into(),
        generation: 1,
        outcome: Outcome::Succeeded as i32,
        request_sha256: "a".repeat(64),
        result: Some(webdock_agent_protocol::receipt::Result::LogsUtf8(
            b"bounded result\n".to_vec(),
        )),
        ..Default::default()
    };
    {
        let mut outbox = ReceiptOutbox::open(&path).unwrap();
        outbox.put(&receipt).unwrap();
    }
    let mut outbox = ReceiptOutbox::open(&path).unwrap();
    assert_eq!(outbox.pending().unwrap(), Some(receipt.clone()));
    let mut different = receipt.clone();
    different.generation = 2;
    assert!(outbox.put(&different).is_err());
    assert!(outbox.acknowledge(&different).is_err());
    outbox.acknowledge(&receipt).unwrap();
    drop(outbox);
    assert!(
        ReceiptOutbox::open(&path)
            .unwrap()
            .pending()
            .unwrap()
            .is_none()
    );
}

#[test]
fn database_checkpoint_delivery_is_exact_and_separate_from_ephemeral_material() {
    use std::os::unix::fs::PermissionsExt;
    use webdock_agent::outbox::DatabaseOutbox;
    use webdock_agent_protocol::{DatabaseCheckpointRequest, DatabaseDelivery, database_delivery};
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.path().join("database-outbox.json");
    let delivery = DatabaseDelivery {
        callback: Some(database_delivery::Callback::Checkpoint(
            DatabaseCheckpointRequest {
                identity: None,
                spec_json: b"{\"operationID\":\"123\"}".to_vec(),
                request_sha256: "a".repeat(64),
                checkpoint_json: b"{\"phase\":\"applied\"}".to_vec(),
            },
        )),
    };
    {
        let mut outbox = DatabaseOutbox::open(&path).unwrap();
        outbox.put(&delivery).unwrap();
    }
    let mut outbox = DatabaseOutbox::open(&path).unwrap();
    assert_eq!(outbox.pending().unwrap(), Some(delivery.clone()));
    let mut changed = delivery.clone();
    if let Some(database_delivery::Callback::Checkpoint(ref mut checkpoint)) = changed.callback {
        checkpoint.request_sha256 = "b".repeat(64);
    }
    assert!(outbox.put(&changed).is_err());
    outbox.acknowledge(&delivery).unwrap();
    assert!(
        DatabaseOutbox::open(&path)
            .unwrap()
            .pending()
            .unwrap()
            .is_none()
    );
}
