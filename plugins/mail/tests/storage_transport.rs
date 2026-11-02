use webdock_mail::store::Store;

#[tokio::test]
async fn plaintext_constructor_rejects_remote_and_driver_host_overrides() {
    for url in [
        "postgresql://remote.invalid/mail",
        "postgresql://localhost/mail?host=remote.invalid",
        "postgresql://localhost/mail?hostaddr=192.0.2.1",
    ] {
        let error = Store::connect(url).await.err().unwrap();
        assert_eq!(
            error.to_string(),
            "database URLs must be loopback until TLS is configured"
        );
    }
}

#[tokio::test]
async fn tls_constructor_rejects_missing_or_invalid_trust_without_downgrade() {
    for ca in [&b""[..], &b"not a CA"[..]] {
        let error = Store::connect_tls("postgresql://remote.invalid/mail?sslmode=disable", ca)
            .await
            .err()
            .unwrap();
        assert_eq!(error.to_string(), "invalid storage TLS trust");
    }
}
