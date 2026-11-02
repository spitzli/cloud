use webdock_postgres::store::Store;
#[tokio::test]
async fn no_tls_store_is_loopback_only_and_tls_never_falls_back() {
    assert_eq!(
        Store::connect("postgresql://hidden:hidden@remote.invalid/metadata")
            .await
            .err()
            .unwrap()
            .to_string(),
        "database URLs must be loopback until TLS is configured"
    );
    assert_eq!(
        Store::connect_tls(
            "postgresql://hidden:hidden@remote.invalid/metadata",
            b"bad CA"
        )
        .await
        .err()
        .unwrap()
        .to_string(),
        "invalid storage TLS trust"
    );
}
