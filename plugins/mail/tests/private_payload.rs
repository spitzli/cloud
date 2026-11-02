use std::sync::Arc;
use webdock_mail::{payload::PgPrivatePayloadStore, store::Store};
use webdock_mail_runtime::PrivatePayloadStore;

#[test]
fn retained_hex_key_is_purpose_bound_and_strict() {
    let a = webdock_mail::payload::payload_key(&"24".repeat(32)).unwrap();
    assert_eq!(
        a,
        webdock_mail::payload::payload_key(&"24".repeat(32)).unwrap()
    );
    assert_ne!(a, "24".repeat(32));
    for bad in ["", "24", &"XX".repeat(32)] {
        assert!(webdock_mail::payload::payload_key(bad).is_err());
    }
}

#[tokio::test]
#[ignore = "requires fresh task3-owned PostgreSQL"]
async fn postgres_immutable_tenant_key_and_retention() {
    let url = std::env::var("WEBDOCK_MAIL_TASK3_TEST_URL").unwrap();
    let ca = std::fs::read(std::env::var("WEBDOCK_MAIL_TASK3_TEST_CA").unwrap()).unwrap();
    let store = Arc::new(Store::connect_tls(&url, &ca).await.unwrap());
    let payload = PgPrivatePayloadStore::new(store.clone(), &"24".repeat(32))
        .await
        .unwrap();
    let reference = uuid::Uuid::new_v4().to_string();
    let saved = payload
        .put("101", &reference, b"immutable payload")
        .await
        .unwrap();
    assert_eq!(saved.reference, reference);
    assert_eq!(saved.bytes, 17);
    assert_eq!(
        payload.get("101", &reference).await.unwrap(),
        b"immutable payload"
    );
    assert!(payload.get("202", &reference).await.is_err());
    assert!(payload.remove("202", &reference).await.is_err());
    assert!(
        payload
            .put("101", &reference, b"replacement")
            .await
            .is_err()
    );
    assert!(
        payload
            .put("202", &reference, b"other tenant")
            .await
            .is_err()
    );
    assert!(
        PgPrivatePayloadStore::new(store.clone(), &"25".repeat(32))
            .await
            .is_err()
    );
    assert!(
        payload
            .put("01", &uuid::Uuid::new_v4().to_string(), b"x")
            .await
            .is_err()
    );
    assert!(payload.put("101", "bad", b"x").await.is_err());
    let domain = webdock_mail::store::Domain {
        id: uuid::Uuid::new_v4(),
        domain: format!("owned-{}.test", uuid::Uuid::new_v4().simple()),
        token: "fixture".into(),
        verified: false,
    };
    store.add_domain("7109", &domain).await.unwrap();
    store.delete_domain("7109", &domain.id).await.unwrap();
    assert_eq!(
        payload.get("101", &reference).await.unwrap(),
        b"immutable payload"
    );
    drop(payload);
    let payload = PgPrivatePayloadStore::new(store.clone(), &"24".repeat(32))
        .await
        .unwrap();
    assert_eq!(
        payload.get("101", &reference).await.unwrap(),
        b"immutable payload"
    );
    let maximum = vec![b'x'; 25 * 1024 * 1024];
    let large = uuid::Uuid::new_v4().to_string();
    payload.put("101", &large, &maximum).await.unwrap();
    assert_eq!(
        payload.get("101", &large).await.unwrap().len(),
        maximum.len()
    );
    assert!(
        payload
            .put(
                "101",
                &uuid::Uuid::new_v4().to_string(),
                &vec![0; 25 * 1024 * 1024 + 1]
            )
            .await
            .is_err()
    );
    let ca = std::fs::read(std::env::var("WEBDOCK_MAIL_TASK3_TEST_CA").unwrap()).unwrap();
    let client = webdock_plugin_kit::postgres::connect(&url, Some(&ca))
        .await
        .unwrap();
    client
        .execute(
            "UPDATE private_payloads SET sha256=$2 WHERE reference=$1",
            &[&uuid::Uuid::parse_str(&large).unwrap(), &"0".repeat(64)],
        )
        .await
        .unwrap();
    assert!(payload.get("101", &large).await.is_err());
    assert!(
        payload.remove("101", &large).await.is_err(),
        "corrupt retained bytes need explicit review"
    );
    payload.remove("101", &reference).await.unwrap();
    payload.remove("101", &reference).await.unwrap();
}
