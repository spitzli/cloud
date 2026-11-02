use super::*;

#[tokio::test]
#[ignore = "owned TLS PostgreSQL; plugins/mail/tests/fixtures/mailbox-recovery.py"]
async fn wrong_token_and_stale_epoch_cannot_release_custody() {
    let url = std::env::var("WEBDOCK_MAIL_RECOVERY_TEST_URL").unwrap();
    let ca = std::fs::read(std::env::var("WEBDOCK_MAIL_RECOVERY_TEST_CA").unwrap()).unwrap();
    let store = Store::connect_tls(&url, &ca).await.unwrap();
    let held = store.recovery_effect().await.unwrap();
    let token = held.token;
    let epoch = held.epoch;
    drop(held);
    for (wrong_token, wrong_epoch) in [(Uuid::new_v4(), epoch), (token, Uuid::new_v4())] {
        let effect = RecoveryEffect {
            guard: store.recovery_guard().await.unwrap(),
            token: wrong_token,
            epoch: wrong_epoch,
        };
        assert!(effect.finish().await.is_err());
    }
    assert!(
        store
            .fence_recovery(epoch, Uuid::new_v4(), 1)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .0
            .query_one(
                "SELECT count(*) FROM mail_recovery_effect WHERE token=$1 AND epoch=$2",
                &[&token, &epoch]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
}
