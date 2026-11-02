//! Private safety interlock tests. These do not manufacture Core backup authority.
use std::sync::Arc;
use uuid::Uuid;
use webdock_mail::{
    payload::PgPrivatePayloadStore,
    store::{Domain, Store},
};
use webdock_mail_runtime::PrivatePayloadStore;

async fn connect(url: &str) -> Store {
    match std::env::var("WEBDOCK_MAIL_RECOVERY_TEST_CA") {
        Ok(path) => Store::connect_tls(url, &std::fs::read(path).unwrap())
            .await
            .unwrap(),
        Err(_) => Store::connect(url).await.unwrap(),
    }
}

#[tokio::test]
#[ignore = "fresh owned private DB; cancelled external effects require real reconciliation"]
async fn cancelled_effect_cannot_be_mistaken_for_a_drained_transport() {
    let url = std::env::var("WEBDOCK_MAIL_RECOVERY_TEST_URL").unwrap();
    let store = connect(&url).await;
    let effect = store.recovery_effect().await.unwrap();
    drop(effect); // A crashed/cancelled sender has no definitive completion witness.
    let epoch = store.recovery_state().await.unwrap().epoch;
    assert!(
        store
            .fence_recovery(epoch, Uuid::new_v4(), 1)
            .await
            .is_err()
    );
    assert_eq!(store.recovery_state().await.unwrap().state, "fenced");
    assert!(store.recovery_effect().await.is_err());
}

#[tokio::test]
#[ignore = "requires fresh owned PostgreSQL; tools/tests/mail-application-recovery.py"]
async fn durable_gate_drains_rejects_stale_resume_and_quarantines_restore() {
    let url = std::env::var("WEBDOCK_MAIL_RECOVERY_TEST_URL").unwrap();
    let store = Arc::new(connect(&url).await);
    let first = store.recovery_state().await.unwrap();
    let payload = PgPrivatePayloadStore::new(store.clone(), &"24".repeat(32))
        .await
        .unwrap();
    let reference = Uuid::new_v4().to_string();
    payload
        .put("101", &reference, b"retained encrypted recovery sentinel")
        .await
        .unwrap();
    let held = store.recovery_effect().await.unwrap();
    let worker = store.clone();
    let operation = Uuid::new_v4();
    let epoch = first.epoch;
    let drain = tokio::spawn(async move { worker.fence_recovery(epoch, operation, 1).await });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while store.recovery_state().await.unwrap().state != "fenced" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !drain.is_finished(),
        "fence acknowledged while a real effect guard is held"
    );
    assert!(store.recovery_guard().await.is_err());
    assert!(
        payload
            .put("101", &Uuid::new_v4().to_string(), b"late")
            .await
            .is_err()
    );
    assert!(payload.remove("101", &reference).await.is_err());
    held.finish().await.unwrap();
    let drained = drain.await.unwrap().unwrap();
    let token = drained.fence().clone();
    assert!(
        store
            .fence_recovery(epoch, Uuid::new_v4(), 1)
            .await
            .is_err()
    );
    let mut stale = token.clone();
    stale.generation += 1;
    assert!(store.resume_recovery(&stale).await.is_err());
    drop(drained);
    let reopened = connect(&url).await;
    assert!(
        reopened.recovery_guard().await.is_err(),
        "crash/restart reopened dispatch"
    );
    assert_eq!(
        payload.get("101", &reference).await.unwrap(),
        b"retained encrypted recovery sentinel"
    );
    assert!(
        PgPrivatePayloadStore::new(store.clone(), &"25".repeat(32))
            .await
            .is_err()
    );
    store.resume_recovery(&token).await.unwrap();
    drop(store.recovery_guard().await.unwrap());
    let drained = store
        .fence_recovery(epoch, Uuid::new_v4(), 2)
        .await
        .unwrap();
    let previous = drained.fence().clone();
    drop(drained);
    let restored_epoch = store.quarantine_recovery(&previous).await.unwrap();
    assert_ne!(restored_epoch, epoch);
    assert!(store.resume_recovery(&previous).await.is_err());
    assert!(store.recovery_guard().await.is_err());
    let domain = Domain {
        id: Uuid::new_v4(),
        domain: "quarantine.example.test".into(),
        token: "local".into(),
        verified: false,
    };
    assert!(store.add_domain("7109", &domain).await.is_err());
    assert_eq!(store.recovery_state().await.unwrap().state, "quarantined");
    assert!(
        store
            .fence_recovery(restored_epoch, Uuid::new_v4(), 3)
            .await
            .is_err()
    );
}

/// Storage-only identity fixtures are not real customer/policy admission evidence.
#[tokio::test]
#[ignore = "owned private snapshot fixture only"]
async fn private_snapshot_prepare() {
    let url = std::env::var("WEBDOCK_MAIL_RECOVERY_TEST_URL").unwrap();
    let key =
        std::fs::read_to_string(std::env::var("WEBDOCK_MAIL_RECOVERY_KEY_FILE").unwrap()).unwrap();
    let store = Arc::new(connect(&url).await);
    let reference = std::env::var("WEBDOCK_MAIL_RECOVERY_REFERENCE").unwrap();
    let payload = PgPrivatePayloadStore::new(store.clone(), &key)
        .await
        .unwrap();
    payload
        .put("101", &reference, b"owned recovery payload sentinel")
        .await
        .unwrap();
    let state = store.recovery_state().await.unwrap();
    let drained = store
        .fence_recovery(state.epoch, Uuid::new_v4(), 1)
        .await
        .unwrap();
    assert_eq!(drained.fence().epoch, state.epoch);
    // Process exit deliberately leaves the durable source fence closed for pg_dump.
}

#[tokio::test]
#[ignore = "owned fresh private restore fixture only"]
async fn private_snapshot_verify() {
    let url = std::env::var("WEBDOCK_MAIL_RECOVERY_TEST_URL").unwrap();
    let key_path = std::env::var("WEBDOCK_MAIL_RECOVERY_KEY_FILE").unwrap();
    assert!(std::fs::read_to_string(format!("{key_path}.missing")).is_err());
    let key = std::fs::read_to_string(key_path).unwrap();
    let store = Arc::new(connect(&url).await);
    let ca = std::fs::read(std::env::var("WEBDOCK_MAIL_RECOVERY_TEST_CA").unwrap()).unwrap();
    let connection = webdock_plugin_kit::postgres::connect(&url, Some(&ca))
        .await
        .unwrap();
    let ownership = connection.query_one("SELECT current_user::text,pg_get_userbyid(datdba)::text,(SELECT ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid()) FROM pg_database WHERE datname=current_database()", &[]).await.unwrap();
    assert_eq!(ownership.get::<_, String>(0), "mail_owner");
    assert_eq!(ownership.get::<_, String>(1), "mail_owner");
    assert!(ownership.get::<_, bool>(2));
    assert!(store.recovery_guard().await.is_err());
    assert_eq!(store.recovery_state().await.unwrap().state, "fenced");
    assert!(
        PgPrivatePayloadStore::new(store.clone(), &"25".repeat(32))
            .await
            .is_err()
    );
    let payload = PgPrivatePayloadStore::new(store.clone(), &key)
        .await
        .unwrap();
    let reference = std::env::var("WEBDOCK_MAIL_RECOVERY_REFERENCE").unwrap();
    assert_eq!(
        payload.get("101", &reference).await.unwrap(),
        b"owned recovery payload sentinel"
    );
    assert!(payload.get("202", &reference).await.is_err());
    assert!(payload.remove("101", &reference).await.is_err());
}
