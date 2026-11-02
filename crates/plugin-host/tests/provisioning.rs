use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, KeyUsagePurpose};
use sha2::{Digest, Sha256};
use webdock_plugin_host::provisioning::{storage::StoragePlan, tls::issue_runtime_identity};

#[test]
fn storage_names_are_stable_bounded_and_platform_scoped() {
    let plan = || {
        StoragePlan::new(
            "physical-platform",
            "postgres",
            "a".repeat(64),
            "storage.test".into(),
            5432,
        )
        .unwrap()
    };
    let a = plan();
    let b = plan();
    assert_eq!(a.database(), b.database());
    assert_eq!(a.role(), b.role());
    let other = StoragePlan::new(
        "other-platform",
        "postgres",
        "a".repeat(64),
        "storage.test".into(),
        5432,
    )
    .unwrap();
    assert_ne!(a.database(), other.database());
    assert_ne!(a.role(), other.role());
    assert!(a.database().len() < 63);
    assert!(
        a.role()
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    );
    assert!(StoragePlan::new("", "postgres", "a".repeat(64), "storage.test".into(), 5432).is_err());
    assert!(
        StoragePlan::new(
            "platform",
            "bad plugin",
            "short".into(),
            "storage.test".into(),
            5432
        )
        .is_err()
    );
}
fn ca() -> (String, String) {
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    (params.self_signed(&key).unwrap().pem(), key.serialize_pem())
}
#[test]
fn runtime_leaf_has_exact_san_both_ekus_and_enrollable_fingerprint() {
    let (cert, key) = ca();
    let identity = issue_runtime_identity(&cert, &key, "runtime.test").unwrap();
    let (_, pem) = x509_parser::pem::parse_x509_pem(identity.certificate_pem.as_bytes()).unwrap();
    let (_, leaf) = x509_parser::parse_x509_certificate(&pem.contents).unwrap();
    assert_eq!(
        identity.certificate_sha256,
        hex::encode(Sha256::digest(&pem.contents))
    );
    let san = &leaf
        .subject_alternative_name()
        .unwrap()
        .unwrap()
        .value
        .general_names;
    assert_eq!(san.len(), 1);
    assert_eq!(
        san[0],
        x509_parser::extensions::GeneralName::DNSName("runtime.test")
    );
    let eku = leaf.extended_key_usage().unwrap().unwrap();
    assert!(eku.value.server_auth && eku.value.client_auth);
    assert!(!leaf.is_ca());
    assert!(leaf.validity().is_valid());
    let (other_cert, other_key) = ca();
    assert!(issue_runtime_identity(&cert, &other_key, "runtime.test").is_err());
    assert!(issue_runtime_identity(&other_cert, &key, "runtime.test").is_err());
    for name in ["*.test", "https://runtime.test", "runtime.test/path", ""] {
        assert!(issue_runtime_identity(&cert, &key, name).is_err());
    }
}

#[path = "support/provisioning_fixture.rs"]
mod fixture;
#[tokio::test]
#[ignore = "requires WEBDOCK_PLUGIN_PROVISIONING_DOCKER_TEST=1; creates only an owned disposable local container"]
async fn tls_login_isolation_and_exact_replay() {
    use webdock_plugin_host::provisioning::{ProvisioningError, storage::MetadataProvisioner};
    let f = fixture::Fixture::start().await;
    let provisioner = MetadataProvisioner::connect(&f.admin_url, Some(f.ca.as_bytes()))
        .await
        .unwrap();
    let (address, port) = provisioner.server_identity().await.unwrap();
    assert!(
        address
            .split('/')
            .next()
            .unwrap()
            .parse::<std::net::IpAddr>()
            .is_ok()
    );
    assert_eq!(
        port, 5432,
        "identity is the PostgreSQL server port, independent of Docker published port"
    );
    let expected = f
        .admin()
        .await
        .query_one("SELECT inet_server_addr()::text,inet_server_port()", &[])
        .await
        .unwrap();
    assert_eq!(address, expected.get::<_, String>(0));
    let plan = StoragePlan::new(
        "platform-one",
        "postgres",
        "a".repeat(64),
        "localhost".into(),
        f.port,
    )
    .unwrap();
    let storage = provisioner.provision(&plan).await.unwrap();
    let app = webdock_plugin_kit::postgres::connect(&storage.url, Some(f.ca.as_bytes()))
        .await
        .unwrap();
    let row = app
        .query_one(
            "SELECT ssl FROM pg_catalog.pg_stat_ssl WHERE pid=pg_catalog.pg_backend_pid()",
            &[],
        )
        .await
        .unwrap();
    assert!(
        row.get::<_, bool>(0),
        "the application connection really uses TLS"
    );
    app.batch_execute("CREATE TABLE retained(value integer); INSERT INTO retained VALUES(42)")
        .await
        .unwrap();
    let repeated = provisioner.provision(&plan).await.unwrap();
    assert!(
        storage.url == repeated.url,
        "replay retains exact private connection material"
    );
    assert_eq!(
        app.query_one("SELECT value FROM retained", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        42
    );
    assert!(
        app.batch_execute("CREATE DATABASE forbidden")
            .await
            .is_err()
    );
    assert!(app.batch_execute("CREATE ROLE forbidden").await.is_err());
    let other_plan = StoragePlan::new(
        "platform-two",
        "postgres",
        "b".repeat(64),
        "localhost".into(),
        f.port,
    )
    .unwrap();
    let other = provisioner.provision(&other_plan).await.unwrap();
    for name in ["postgres", "template1", other.database()] {
        let mut uri = url::Url::parse(&storage.url).unwrap();
        uri.set_path(name);
        assert!(
            webdock_plugin_kit::postgres::connect(uri.as_str(), Some(f.ca.as_bytes()))
                .await
                .is_err()
        );
    }
    let changed = StoragePlan::new(
        "platform-one",
        "postgres",
        "c".repeat(64),
        "localhost".into(),
        f.port,
    )
    .unwrap();
    assert_eq!(
        provisioner.provision(&changed).await.err(),
        Some(ProvisioningError::Ownership)
    );
    assert!(
        webdock_plugin_kit::postgres::connect(&storage.url, Some(f.ca.as_bytes()))
            .await
            .is_ok()
    );
    let (wrong_ca, _) = ca();
    assert!(
        MetadataProvisioner::connect(&f.admin_url, Some(wrong_ca.as_bytes()))
            .await
            .is_err()
    );
    let mut wrong_name = url::Url::parse(&f.admin_url).unwrap();
    wrong_name.set_host(Some("127.0.0.1")).unwrap();
    assert!(
        MetadataProvisioner::connect(wrong_name.as_str(), Some(f.ca.as_bytes()))
            .await
            .is_err()
    );
    let admin = f.admin().await;
    let foreign = StoragePlan::new(
        "foreign",
        "postgres",
        "d".repeat(64),
        "localhost".into(),
        f.port,
    )
    .unwrap();
    admin
        .batch_execute(&format!("CREATE ROLE {} LOGIN", foreign.role()))
        .await
        .unwrap();
    assert_eq!(
        provisioner.provision(&foreign).await.err(),
        Some(ProvisioningError::Ownership)
    );
    admin
        .batch_execute(&format!("COMMENT ON DATABASE {} IS NULL", plan.database()))
        .await
        .unwrap();
    assert!(
        provisioner.provision(&plan).await.is_ok(),
        "recovery after CREATE DATABASE preserves the owned database"
    );
    let bad_runtime_host = StoragePlan::new(
        "wrong-runtime-host",
        "postgres",
        "e".repeat(64),
        "127.0.0.1".into(),
        f.port,
    )
    .unwrap();
    let bad = provisioner.provision(&bad_runtime_host).await.unwrap();
    assert!(
        webdock_plugin_kit::postgres::connect(&bad.url, Some(f.ca.as_bytes()))
            .await
            .is_err(),
        "the runtime independently verifies its private endpoint hostname"
    );
}

#[path = "support/runtime_fixture.rs"]
mod runtime_fixture;
#[tokio::test]
#[ignore = "requires owned Docker fixture and WEBDOCK_POSTGRES_RUNTIME_BINARY pointing to the locally built PostgreSQL runtime"]
async fn actual_postgres_binary_starts_over_pinned_mtls_and_private_unix() {
    use runtime_fixture::{Child, DenyHost, private};
    use webdock_plugin_host::provisioning::storage::MetadataProvisioner;
    use webdock_plugin_runtime::{PinnedMutualTls, connect_pinned_mtls, wire};
    let binary =
        std::env::var_os("WEBDOCK_POSTGRES_RUNTIME_BINARY").expect("explicit owned test binary");
    let f = fixture::Fixture::start().await;
    let directory = tempfile::tempdir_in(f.directory.path()).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = directory.path();
    let provisioner = MetadataProvisioner::connect(&f.admin_url, Some(f.ca.as_bytes()))
        .await
        .unwrap();
    let plan = StoragePlan::new(
        "standalone-runtime",
        "postgres",
        "f".repeat(64),
        "localhost".into(),
        f.port,
    )
    .unwrap();
    let storage = provisioner.provision(&plan).await.unwrap();
    let core = issue_runtime_identity(&f.ca, &f.ca_key, "localhost").unwrap();
    let runtime = issue_runtime_identity(&f.ca, &f.ca_key, "localhost").unwrap();
    for (file, value) in [
        ("runtime.crt", runtime.certificate_pem.as_str()),
        ("runtime.key", runtime.private_key_pem.as_str()),
        ("core-ca.crt", f.ca.as_str()),
        ("storage-ca.crt", f.ca.as_str()),
    ] {
        private(&path.join(file), value);
    }
    let manifest = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/postgres/manifest.json"
    ))
    .unwrap();
    private(&path.join("manifest.json"), &manifest);
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let core_port = listener.local_addr().unwrap().port();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let stream = futures_util::stream::unfold(listener, |listener| async move {
        let accepted = listener.accept().await.map(|(io, _)| io);
        Some((accepted, listener))
    });
    let server = tokio::spawn(
        tonic::transport::Server::builder()
            .tls_config(
                tonic::transport::ServerTlsConfig::new()
                    .identity(tonic::transport::Identity::from_pem(
                        &core.certificate_pem,
                        &core.private_key_pem,
                    ))
                    .client_ca_root(tonic::transport::Certificate::from_pem(&f.ca)),
            )
            .unwrap()
            .add_service(wire::plugin_host_server::PluginHostServer::new(DenyHost))
            .serve_with_incoming_shutdown(stream, async {
                let _ = stop_rx.await;
            }),
    );
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let runtime_port = free.local_addr().unwrap().port();
    drop(free);
    let mut config = serde_json::json!({"manifest":path.join("manifest.json"),"issuerPublicKeyHex":hex::encode(ed25519_dalek::SigningKey::from_bytes(&[71;32]).verifying_key().as_bytes()),"generation":1,
        "listenTls":{"address":format!("127.0.0.1:{runtime_port}"),"certificate":path.join("runtime.crt"),"privateKey":path.join("runtime.key"),"authority":path.join("core-ca.crt")},
        "coreTls":{"origin":format!("https://localhost:{core_port}"),"domain":"localhost","certificate":path.join("runtime.crt"),"privateKey":path.join("runtime.key"),"authority":path.join("core-ca.crt"),"serverCertificateSha256":core.certificate_sha256},
        "corePeers":[{"kind":"mutualTls","certificateSha256":core.certificate_sha256}],"storageUrl":storage.url,"storageCa":path.join("storage-ca.crt"),"secretKeyHex":"7a".repeat(32),"sharedServers":[],"dedicated":{"domain":"db.test","port":5432,"entryPoints":["postgres"]}});
    let config_path = path.join("runtime.json");
    let start = |config: &serde_json::Value| {
        private(&config_path, serde_json::to_vec(config).unwrap());
        Child(
            std::process::Command::new(&binary)
                .arg(&config_path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        )
    };
    let tls = || PinnedMutualTls {
        certificate: core.certificate_pem.as_bytes().to_vec(),
        private_key: core.private_key_pem.as_bytes().to_vec(),
        authority: f.ca.as_bytes().to_vec(),
        server_certificate_sha256: runtime.certificate_sha256.clone(),
    };
    let origin = format!("https://localhost:{runtime_port}");
    let mut child = start(&config);
    let mut connected = None;
    for attempt in 0..100 {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "PostgreSQL binary exited before readiness (private diagnostics suppressed)"
        );
        if let Ok(client) = connect_pinned_mtls(origin.clone(), "localhost".into(), tls()).await {
            connected = Some(client);
            break;
        }
        assert!(attempt < 99, "PostgreSQL binary did not become ready");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let mut client = connected.unwrap();
    let health = client
        .health(wire::HealthRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        (
            health.plugin_id.as_str(),
            health.generation,
            health.protocol_version
        ),
        ("postgres", 1, 1)
    );
    assert_eq!(
        client
            .describe(wire::DescribeRequest {})
            .await
            .unwrap()
            .into_inner()
            .manifest_json,
        manifest
    );
    let app = webdock_plugin_kit::postgres::connect(
        config["storageUrl"].as_str().unwrap(),
        Some(f.ca.as_bytes()),
    )
    .await
    .unwrap();
    assert_eq!(
        app.query_one("SELECT max(version) FROM schema_migrations", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        3
    );
    app.batch_execute(
        "CREATE TABLE runtime_retention(value integer); INSERT INTO runtime_retention VALUES(73)",
    )
    .await
    .unwrap();
    let mut wrong_pin = tls();
    wrong_pin.server_certificate_sha256 = "00".repeat(32);
    assert!(
        connect_pinned_mtls(origin.clone(), "localhost".into(), wrong_pin)
            .await
            .is_err()
    );
    assert!(
        connect_pinned_mtls(origin.clone(), "wrong.test".into(), tls())
            .await
            .is_err()
    );
    let other = issue_runtime_identity(&f.ca, &f.ca_key, "localhost").unwrap();
    let mut unapproved = connect_pinned_mtls(
        origin,
        "localhost".into(),
        PinnedMutualTls {
            certificate: other.certificate_pem.into_bytes(),
            private_key: other.private_key_pem.into_bytes(),
            authority: f.ca.as_bytes().to_vec(),
            server_certificate_sha256: runtime.certificate_sha256,
        },
    )
    .await
    .unwrap();
    assert!(
        unapproved.health(wire::HealthRequest {}).await.is_err(),
        "same CA never grants an unpinned Core probe identity"
    );
    drop(client);
    drop(child);
    let mut core_socket = webdock_plugin_runtime::UnixSocket::bind(path.join("core.sock")).unwrap();
    let incoming = core_socket.incoming().unwrap();
    let (unix_stop_tx, unix_stop_rx) = tokio::sync::oneshot::channel::<()>();
    let unix_server = tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(wire::plugin_host_server::PluginHostServer::new(DenyHost))
            .serve_with_incoming_shutdown(incoming, async {
                let _ = unix_stop_rx.await;
            }),
    );
    config.as_object_mut().unwrap().remove("listenTls");
    config.as_object_mut().unwrap().remove("coreTls");
    config["listen"] = serde_json::to_value(path.join("plugin.sock")).unwrap();
    config["core"] = serde_json::to_value(path.join("core.sock")).unwrap();
    config["corePeers"] =
        serde_json::json!([{"kind":"unix","uid":rustix::process::geteuid().as_raw()}]);
    let mut child = start(&config);
    let mut unix_client = None;
    for attempt in 0..100 {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "private Unix runtime exited (private diagnostics suppressed)"
        );
        if let Ok(c) = webdock_plugin_runtime::connect_unix(path.join("plugin.sock")).await {
            unix_client = Some(c);
            break;
        }
        assert!(attempt < 99, "private Unix runtime did not become ready");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(
        unix_client
            .unwrap()
            .health(wire::HealthRequest {})
            .await
            .unwrap()
            .into_inner()
            .plugin_id,
        "postgres"
    );
    assert_eq!(
        app.query_one("SELECT value FROM runtime_retention", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        73
    );
    drop(child);
    let _ = unix_stop_tx.send(());
    unix_server.await.unwrap().unwrap();
    drop(core_socket);
    // Startup diagnostics contain no underlying private URL, password or driver error.
    config["storageUrl"] =
        "postgresql://very-private-user:very-private-password@remote.invalid/metadata".into();
    config["storageCa"] = path.join("bad-ca.crt").to_str().unwrap().into();
    private(&path.join("bad-ca.crt"), "invalid CA");
    private(&config_path, serde_json::to_vec(&config).unwrap());
    let output = std::process::Command::new(&binary)
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stderr, b"postgres_plugin_start_failed\n");
    let _ = stop_tx.send(());
    server.await.unwrap().unwrap();
}
