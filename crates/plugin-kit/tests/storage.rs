#[tokio::test]
async fn plaintext_storage_rejects_remote_and_host_overrides_before_connecting() {
    for url in [
        "postgresql://secret:secret@remote.invalid/db",
        "postgresql://u:p@localhost/db?host=remote.invalid",
        "postgresql://u:p@localhost/db?hostaddr=192.0.2.1",
    ] {
        let error = webdock_plugin_kit::postgres::connect(url, None)
            .await
            .err()
            .unwrap();
        assert_eq!(error.to_string(), "plaintext storage requires loopback");
    }
}
#[tokio::test]
async fn invalid_or_missing_tls_trust_fails_without_plaintext_fallback() {
    for ca in [&b""[..], &b"not a CA"[..]] {
        let error = webdock_plugin_kit::postgres::connect(
            "postgresql://secret:secret@remote.invalid/db?sslmode=disable",
            Some(ca),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.to_string(), "invalid storage TLS trust");
    }
}
