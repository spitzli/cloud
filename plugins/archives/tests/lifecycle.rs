use serde_json::json;
use webdock_archives::{
    diagnostics::Diagnostics,
    model::{Archive, Kind, State},
};
use webdock_plugin_protocol::storage::{StorageAuthorization, StorageDirection};
fn archive() -> Archive {
    Archive { id:uuid::Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(), customer:"123".into(),project:"124".into(),kind:Kind::Diagnostics,state:State::Ready,epoch:uuid::Uuid::new_v4().to_string(),expires_at:2000,source_binding:None,core_source:None,failure:None,job:Some("job-example".into()),receipt:Some(webdock_object_storage::ObjectReceipt {backend_id:"owned".into(),binding:"a".repeat(64),object_key:"archives/123/124/11111111-1111-4111-8111-111111111111/22222222-2222-4222-8222-222222222222".into(),size_bytes:42,sha256:"b".repeat(64)}),diagnostics:Some(Diagnostics::parse(json!({"version":1,"state":"finished","builds":[],"checks":[]})).unwrap()) }
}
fn auth(r: &Archive) -> StorageAuthorization {
    StorageAuthorization {
        version: 1,
        transfer_id: uuid::Uuid::new_v4().to_string(),
        source_invocation: uuid::Uuid::new_v4().to_string(),
        plugin_id: "archives".into(),
        generation: 1,
        action: "archives.download".into(),
        customer_id: r.customer.clone(),
        project_id: r.project.clone(),
        resource_id: r.id.to_string(),
        resource_epoch: r.epoch.clone(),
        direction: StorageDirection::Download,
        exact_bytes: 42,
        expected_sha256: Some("b".repeat(64)),
        expires_at: 1900,
    }
}
#[test]
fn download_is_bound_to_the_exact_ready_receipt_tenant_epoch_and_expiry() {
    let r = archive();
    let a = auth(&r);
    assert!(r.authorize(&a, 1000).is_ok());
    for change in 0..8 {
        let mut a = a.clone();
        match change {
            0 => a.customer_id = "999".into(),
            1 => a.project_id = "999".into(),
            2 => a.resource_epoch = uuid::Uuid::new_v4().to_string(),
            3 => a.resource_id = uuid::Uuid::new_v4().to_string(),
            4 => a.exact_bytes = 43,
            5 => a.expected_sha256 = Some("c".repeat(64)),
            6 => a.direction = StorageDirection::Upload,
            _ => a.action = "other.download".into(),
        };
        assert!(
            r.authorize(&a, 1000).is_err(),
            "authority mutation {change}"
        );
    }
    assert!(r.authorize(&a, 2000).is_err());
    for state in [
        State::Pending,
        State::Producing,
        State::Deleting,
        State::Deleted,
        State::Failed,
    ] {
        let mut r = archive();
        let a = auth(&r);
        r.state = state;
        assert!(r.authorize(&a, 1000).is_err());
    }
}

#[test]
fn historical_database_receipts_without_managed_source_never_authorize_bytes() {
    let mut legacy = archive();
    legacy.kind = Kind::Postgres;
    legacy.source_binding = Some("a".repeat(64));
    let authorization = auth(&legacy);
    assert!(
        legacy.ready(1000).is_err(),
        "legacy receipt must not become a download plan"
    );
    assert!(
        legacy.authorize(&authorization, 1000).is_err(),
        "issued legacy transfer must fail closed"
    );
    assert!(
        legacy.owned("123", "124").is_ok(),
        "ownership remains available for exact deletion"
    );
}
