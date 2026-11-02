use webdock_plugin_kit::transport::RuntimeTransport;

#[test]
fn exactly_one_listener_and_callback_are_required() {
    let unix = serde_json::json!({"listen":"/run/plugin.sock","core":"/run/core.sock"});
    let parsed: RuntimeTransport = serde_json::from_value(unix.clone()).unwrap();
    assert!(parsed.validate().is_ok());
    for bad in [
        serde_json::json!({}),
        serde_json::json!({"listen":"relative","core":"/run/core.sock"}),
        serde_json::json!({"listen":"/run/plugin.sock"}),
        serde_json::json!({"listen":"/run/plugin.sock","core":"/run/core.sock","coreTls":{"origin":"https://core.test","domain":"core.test","certificate":"/run/cert","privateKey":"/run/key","authority":"/run/ca","serverCertificateSha256":"a".repeat(64)}}),
    ] {
        let value: RuntimeTransport = serde_json::from_value(bad).unwrap();
        assert!(value.validate().is_err());
    }
}

#[test]
fn tls_callback_is_an_exact_https_origin_with_owned_files() {
    let good = serde_json::json!({"listenTls":{"address":"0.0.0.0:8443","certificate":"/run/cert","privateKey":"/run/key","authority":"/run/ca"},"coreTls":{"origin":"https://core.test:8443","domain":"core.test","certificate":"/run/cert","privateKey":"/run/key","authority":"/run/ca","serverCertificateSha256":"a".repeat(64)}});
    assert!(
        serde_json::from_value::<RuntimeTransport>(good.clone())
            .unwrap()
            .validate()
            .is_ok()
    );
    for (field, value) in [
        ("origin", "http://core.test"),
        ("origin", "https://core.test/path"),
        ("origin", "https://user:pass@core.test"),
        ("domain", "other.test"),
        ("serverCertificateSha256", "bad"),
        ("privateKey", "relative"),
    ] {
        let mut bad = good.clone();
        bad["coreTls"][field] = value.into();
        assert!(
            serde_json::from_value::<RuntimeTransport>(bad)
                .unwrap()
                .validate()
                .is_err()
        );
    }
}

#[tokio::test]
async fn lazy_core_allows_missing_socket_but_preserves_private_directory_checks() {
    use std::os::unix::fs::DirBuilderExt;
    let directory =
        std::env::temp_dir().join(format!("kit-lazy-{}", uuid::Uuid::new_v4().simple()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .unwrap();
    let transport = RuntimeTransport {
        listen: Some(directory.join("plugin.sock")),
        core: Some(directory.join("core.sock")),
        ..RuntimeTransport::default()
    };
    assert!(transport.core_client().await.is_err());
    let result = transport.core_client_lazy().await;
    std::fs::remove_dir(&directory).unwrap();
    assert!(
        result.is_ok(),
        "callback availability must not gate listener construction"
    );
    assert!(
        transport.core_client_lazy().await.is_err(),
        "missing private parent is not a valid socket configuration"
    );
}
