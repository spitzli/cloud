use webdock_databases::database_connection_url;
#[test]
fn application_binding_uses_fixed_ca_mount_and_verified_tls() {
    let url = database_connection_url(
        "postgres-rw.wd-db-123.svc.cluster.local",
        5432,
        "db_123",
        "wd_app_123",
        "fixture-password",
        "456",
    )
    .unwrap();
    let parsed = url::Url::parse(&url).unwrap();
    assert_eq!(parsed.scheme(), "postgresql");
    assert_eq!(parsed.username(), "wd_app_123");
    assert_eq!(parsed.path(), "/db_123");
    let query = parsed
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(query["sslmode"], "verify-full");
    assert_eq!(query["sslrootcert"], "/var/run/webdock/services/456/ca.crt");
    for host in ["host/path", "user@host", "127.0.0.1:5432", "UPPER.invalid"] {
        assert!(
            database_connection_url(
                host,
                5432,
                "db_123",
                "wd_app_123",
                "fixture-password",
                "456"
            )
            .is_err()
        );
    }
    assert!(
        database_connection_url(
            "host.internal",
            5432,
            "db_123",
            "wd_app_123",
            "fixture-password",
            "../ca"
        )
        .is_err()
    );
}
