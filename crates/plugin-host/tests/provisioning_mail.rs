//! Focused process proof. Host/JMAP stand-ins are not native Core/CE/Console acceptance.
#[path = "support/mail_runtime.rs"]
mod fixture;
use fixture::*;
use serde_json::{Value, json};
use std::{sync::atomic::Ordering, time::Duration};
use webdock_plugin_runtime::{HostClient, PinnedMutualTls, connect_pinned_mtls, wire};

fn retained_pvc(input: &Value) -> bool {
    input["objects"].as_array().unwrap().iter().any(|object| {
        object["kind"] == "PersistentVolumeClaim" && object["metadata"]["name"] == "engine-data"
    })
}

#[tokio::test]
#[ignore = "owned disposable TLS PostgreSQL; requires WEBDOCK_PLUGIN_PROVISIONING_DOCKER_TEST=1 and WEBDOCK_MAIL_RUNTIME_BINARY"]
async fn mail_binary_tls_callbacks_unix_restart_and_private_diagnostics() {
    let mut f = Fixture::new().await;
    f.calls.active.store(1, Ordering::SeqCst);
    let mut child = f.start(&f.config);
    let mut client = f.client(&mut child).await;
    assert_eq!(
        client
            .describe(wire::DescribeRequest {})
            .await
            .unwrap()
            .into_inner()
            .manifest_json,
        include_bytes!("../../../plugins/mail/manifest.json")
    );
    let created: Value = serde_json::from_slice(
        &client
            .invoke(invoke(
                1,
                "domains.create",
                json!({"domain":"retained.example.test"}),
            ))
            .await
            .unwrap()
            .into_inner()
            .output_json,
    )
    .unwrap();
    let domain = created["resource"]["id"].as_str().unwrap();
    client.invoke(invoke(1,"relay.connect",json!({"consumer_key":"fixture-only-unused-key","consumer_secret":"fixture-only-unused-secret"}))).await.unwrap();
    assert!(f.calls.invocations.load(Ordering::SeqCst) > 0);
    assert!(f.calls.payloads.load(Ordering::SeqCst) > 0);
    assert!(f.calls.workloads.lock().unwrap().is_empty());
    assert_eq!(
        f.sql
            .query_one("SELECT count(*) FROM customer_engines", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    assert_eq!(
        f.sql
            .query_one("SELECT count(*) FROM mailboxes", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    let versions: Vec<i32> = f
        .sql
        .query(
            "SELECT version FROM schema_migrations ORDER BY version",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect();
    assert_eq!(versions, [1, 2, 3, 4]);
    assert!(
        f.sql
            .query_one(
                "SELECT ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid()",
                &[]
            )
            .await
            .unwrap()
            .get::<_, bool>(0)
    );
    let before = f.snapshot().await;
    let origin = format!("https://localhost:{}", f.port);
    let mut wrong_ca = f.tls();
    wrong_ca.authority = f.pg.ca.as_bytes().to_vec();
    assert!(
        connect_pinned_mtls(origin.clone(), "localhost".into(), wrong_ca)
            .await
            .is_err()
    );
    let mut wrong = f.tls();
    wrong.server_certificate_sha256 = "00".repeat(32);
    assert!(
        connect_pinned_mtls(origin.clone(), "localhost".into(), wrong)
            .await
            .is_err()
    );
    assert!(
        connect_pinned_mtls(origin.clone(), "wrong.test".into(), f.tls())
            .await
            .is_err()
    );
    let other = webdock_plugin_host::provisioning::tls::issue_runtime_identity(
        &f.ca,
        &f.ca_key,
        "localhost",
    )
    .unwrap();
    let mut alien = connect_pinned_mtls(
        origin.clone(),
        "localhost".into(),
        PinnedMutualTls {
            certificate: other.certificate_pem.as_bytes().to_vec(),
            private_key: other.private_key_pem.as_bytes().to_vec(),
            authority: f.ca.as_bytes().to_vec(),
            server_certificate_sha256: f.runtime.certificate_sha256.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        alien
            .health(wire::HealthRequest {})
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    // Same CA is not runtime enrollment at the callback either.
    let callback = webdock_plugin_runtime::pinned_mtls_channel(
        f.config["coreTls"]["origin"].as_str().unwrap().into(),
        "localhost".into(),
        PinnedMutualTls {
            certificate: other.certificate_pem.as_bytes().to_vec(),
            private_key: other.private_key_pem.as_bytes().to_vec(),
            authority: f.ca.as_bytes().to_vec(),
            server_certificate_sha256: f.core.certificate_sha256.clone(),
        },
    )
    .await
    .unwrap();
    assert!(
        HostClient::new(callback)
            .authorize_runtime("mail", 1)
            .await
            .is_err()
    );
    drop(alien);
    drop(client);
    shutdown(&mut child).await;
    drop(child);
    let unix = f.unix().await;
    for _ in 0..2 {
        let mut child = f.start(&unix);
        let mut client = None;
        for _ in 0..100 {
            if let Ok(c) =
                webdock_plugin_runtime::connect_unix(f.directory.path().join("mail.sock")).await
            {
                client = Some(c);
                break;
            }
            assert!(child.0.try_wait().unwrap().is_none(), "Unix Mail exited");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let mut client = client.expect("Unix listener deadline");
        assert_eq!(
            client
                .health(wire::HealthRequest {})
                .await
                .unwrap()
                .into_inner()
                .plugin_id,
            "mail"
        );
        client
            .invoke(invoke(1, "domains.view", json!({"resource_id":domain})))
            .await
            .unwrap();
        assert!(
            before == f.snapshot().await,
            "Unix restart changed retained rows or ciphertext"
        );
        let versions: Vec<i32> = f
            .sql
            .query(
                "SELECT version FROM schema_migrations ORDER BY version",
                &[],
            )
            .await
            .unwrap()
            .iter()
            .map(|r| r.get(0))
            .collect();
        assert_eq!(versions, [1, 2, 3, 4]);
        drop(client);
        shutdown(&mut child).await;
        drop(child);
    }
    // Each bad configuration reaches the real executable and emits one fixed marker.
    for fault in [
        "storage-ca",
        "storage-name",
        "callback-ca",
        "ambiguous",
        "private-mode",
    ] {
        let mut bad = f.config.clone();
        private(&f.directory.path().join("bad-ca.crt"), "not a CA");
        private(&f.directory.path().join("wrong-storage-ca.crt"), &f.ca);
        match fault {
            "storage-ca" => {
                bad["storageCa"] = json!(f.directory.path().join("wrong-storage-ca.crt"))
            }
            "storage-name" => {
                let mut url = url::Url::parse(bad["storageUrl"].as_str().unwrap()).unwrap();
                url.set_host(Some("127.0.0.1")).unwrap();
                bad["storageUrl"] = json!(url.as_str());
            }
            "callback-ca" => {
                bad["coreTls"]["authority"] = json!(f.directory.path().join("bad-ca.crt"))
            }
            "ambiguous" => bad["listen"] = json!(f.directory.path().join("forbidden.sock")),
            _ => {}
        }
        let mut child = if fault == "private-mode" {
            use std::os::unix::fs::PermissionsExt;
            let path = config_path(f.directory.path());
            private(&path, serde_json::to_vec(&bad).unwrap());
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            let process = std::process::Command::new(&f.binary)
                .arg(path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            // Fixture::start owns the standard child wrapper; preserve the same cleanup contract.
            mail_child(process)
        } else {
            f.start(&bad)
        };
        wait_until(async || child.0.try_wait().unwrap().is_some()).await;
        assert!(
            !child.0.try_wait().unwrap().unwrap().success(),
            "bad private configuration started"
        );
        use std::io::Read;
        let mut stderr = Vec::new();
        child
            .0
            .stderr
            .take()
            .unwrap()
            .read_to_end(&mut stderr)
            .unwrap();
        assert!(
            stderr == b"mail_plugin_start_failed\n",
            "unexpected startup diagnostics; contents withheld"
        );
        if fault == "private-mode" {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                config_path(f.directory.path()),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "owned disposable TLS PostgreSQL and actual Mail child; Host/JMAP stand-ins only"]
async fn candidate_retained_intent_has_zero_effects_until_active_then_resumes_and_restarts() {
    let mut f = Fixture::new().await;
    // Initialize only physical schema with the actual candidate executable, then stop it.
    let mut initial = f.start(&f.config);
    let client = f.client(&mut initial).await;
    drop(client);
    shutdown(&mut initial).await;
    drop(initial);
    seed(&f.sql).await;
    let (origin, jmap_server) = jmap(f.calls.clone()).await;
    f.servers.push(jmap_server);
    f.config["mailboxes"] = mailboxes(&origin);
    let original_secrets:String=f.sql.query_one("SELECT jsonb_build_object('engines',(SELECT jsonb_agg(jsonb_build_array(customer_id,id,admin_sealed) ORDER BY customer_id) FROM customer_engines),'mailboxes',(SELECT jsonb_agg(jsonb_build_array(id,password_sealed,account_id) ORDER BY id) FROM mailboxes))::text",&[]).await.unwrap().get(0);
    for generation in [2, 3] {
        f.calls.active.store(0, Ordering::SeqCst);
        f.config["generation"] = json!(generation);
        f.calls.workloads.lock().unwrap().clear();
        f.calls.jmap.lock().unwrap().clear();
        let before = f.snapshot().await;
        let peer_calls = f.calls.peers.load(Ordering::SeqCst);
        let mut child = f.start(&f.config);
        let mut client = f.client(&mut child).await;
        let health = client
            .health(wire::HealthRequest {})
            .await
            .unwrap()
            .into_inner();
        assert_eq!(health.generation, generation);
        client.describe(wire::DescribeRequest {}).await.unwrap();
        assert_eq!(
            client
                .invoke(invoke(
                    generation,
                    "domains.create",
                    json!({"domain":"candidate.example.test"})
                ))
                .await
                .unwrap_err()
                .code(),
            tonic::Code::PermissionDenied
        );
        let callback = webdock_plugin_runtime::pinned_mtls_channel(
            f.config["coreTls"]["origin"].as_str().unwrap().into(),
            "localhost".into(),
            PinnedMutualTls {
                certificate: f.runtime.certificate_pem.as_bytes().to_vec(),
                private_key: f.runtime.private_key_pem.as_bytes().to_vec(),
                authority: f.ca.as_bytes().to_vec(),
                server_certificate_sha256: f.core.certificate_sha256.clone(),
            },
        )
        .await
        .unwrap();
        assert!(
            HostClient::new(callback)
                .workloads("mail", generation, "observe", json!({"unit":"engine-c101"}))
                .await
                .is_err()
        );
        wait_until(async || f.calls.peers.load(Ordering::SeqCst) >= peer_calls + 3).await;
        assert!(
            f.calls.jmap.lock().unwrap().is_empty(),
            "candidate contacted customer JMAP"
        );
        assert!(
            f.calls.workloads.lock().unwrap().is_empty(),
            "candidate changed workloads"
        );
        assert!(
            before == f.snapshot().await,
            "candidate changed retained state or secrets"
        );
        f.calls.active.store(generation, Ordering::SeqCst);
        wait_until(async ||{let rows=f.sql.query("SELECT state FROM customer_engines ORDER BY customer_id",&[]).await.unwrap();rows.iter().map(|r|r.get::<_,String>(0)).collect::<Vec<_>>()==["ready","ready","suspended"] && f.calls.workloads.lock().unwrap().iter().filter(|(m,_)|m=="apply").count()>=3 && f.sql.query_one("SELECT count(*) FROM mailboxes WHERE customer_id IN ('101','102') AND state='ready'",&[]).await.unwrap().get::<_,i64>(0)==2}).await;
        {
            let workloads = f.calls.workloads.lock().unwrap();
            assert!(
                workloads
                    .iter()
                    .all(|(method, input)| method != "remove" && input["project"].is_null())
            );
            for customer in ["101", "102", "103"] {
                assert!(workloads.iter().any(|(method, input)| {
                    method == "apply"
                        && input["customer"] == customer
                        && input["unit"] == format!("engine-c{customer}")
                        && retained_pvc(input)
                }));
            }
            assert!(workloads.iter().any(|(_, i)| i["customer"] == "103"
                && i["objects"].as_array().is_some_and(|o| {
                    o.iter()
                        .any(|o| o["kind"] == "Deployment" && o["spec"]["replicas"] == 0)
                })));
        }
        assert!(!f.calls.jmap.lock().unwrap().is_empty());
        assert!(
            f.calls
                .jmap
                .lock()
                .unwrap()
                .iter()
                .all(|(host, _)| !host.starts_with("c103.")),
            "suspended intent contacted JMAP"
        );
        let retained:String=f.sql.query_one("SELECT jsonb_build_object('engines',(SELECT jsonb_agg(jsonb_build_array(customer_id,id,admin_sealed) ORDER BY customer_id) FROM customer_engines),'mailboxes',(SELECT jsonb_agg(jsonb_build_array(id,password_sealed,account_id) ORDER BY id) FROM mailboxes))::text",&[]).await.unwrap().get(0);
        assert!(
            original_secrets == retained,
            "reconciliation replaced credentials/account identity"
        );
        drop(client);
        shutdown(&mut child).await;
        drop(child);
    }
    // A candidate waiting forever must shut down without reconciliation, even after later activation.
    f.calls.active.store(0, Ordering::SeqCst);
    f.config["generation"] = json!(4);
    f.calls.workloads.lock().unwrap().clear();
    f.calls.jmap.lock().unwrap().clear();
    let before = f.snapshot().await;
    let peers = f.calls.peers.load(Ordering::SeqCst);
    let mut child = f.start(&f.config);
    drop(f.client(&mut child).await);
    wait_until(async || f.calls.peers.load(Ordering::SeqCst) > peers).await;
    shutdown(&mut child).await;
    drop(child);
    f.calls.active.store(4, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(before == f.snapshot().await);
    assert!(f.calls.jmap.lock().unwrap().is_empty());
    assert!(f.calls.workloads.lock().unwrap().is_empty());
}

#[tokio::test]
#[ignore = "owned TLS PostgreSQL and actual Mail process; no external callback or engine"]
async fn initial_callback_offline_recovers_and_bad_callback_identity_never_resumes() {
    let mut f = Fixture::new().await;
    let mut initial = f.start(&f.config);
    drop(f.client(&mut initial).await);
    shutdown(&mut initial).await;
    drop(initial);
    seed(&f.sql).await;
    let (origin, jmap_server) = jmap(f.calls.clone()).await;
    f.servers.push(jmap_server);
    f.config["mailboxes"] = mailboxes(&origin);
    let before = f.snapshot().await;
    // Stop only our owned callback, retaining the exact same endpoint and identity for recovery.
    f.servers.remove(0).0.abort();
    let callback_url = url::Url::parse(f.config["coreTls"]["origin"].as_str().unwrap()).unwrap();
    let address = format!("127.0.0.1:{}", callback_url.port().unwrap());
    let listener = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(listener) = tokio::net::TcpListener::bind(&address).await {
                break listener;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned callback did not stop");
    // Reserve the port without accepting a TLS connection: callback startup is genuinely offline.
    drop(listener);
    let mut child = f.start(&f.config);
    let mut client = f.client(&mut child).await;
    assert!(
        client
            .invoke(invoke(
                1,
                "domains.create",
                json!({"domain":"offline.example.test"})
            ))
            .await
            .is_err()
    );
    assert!(before == f.snapshot().await);
    assert!(f.calls.jmap.lock().unwrap().is_empty());
    assert!(f.calls.workloads.lock().unwrap().is_empty());
    let listener = tokio::net::TcpListener::bind(&address).await.unwrap();
    f.calls.active.store(1, Ordering::SeqCst);
    f.servers.push(callback(
        listener,
        Host {
            calls: f.calls.clone(),
            runtime_pin: f.runtime.certificate_sha256.clone(),
            unix: false,
        },
        &f.core,
        &f.ca,
    ));
    wait_until(async || !f.calls.jmap.lock().unwrap().is_empty()).await;
    drop(client);
    shutdown(&mut child).await;
    drop(child);
    // Valid TLS material with the wrong expected server name/pin must never authorize startup.
    for fault in ["callback-name", "callback-pin", "callback-valid-ca"] {
        let mut bad = f.config.clone();
        if fault == "callback-valid-ca" {
            bad["coreTls"]["authority"] = json!(f.directory.path().join("storage-ca.crt"));
        } else if fault == "callback-pin" {
            bad["coreTls"]["serverCertificateSha256"] = json!("00".repeat(32));
        } else {
            bad["coreTls"]["domain"] = json!("127.0.0.1");
            bad["coreTls"]["origin"] = json!(format!(
                "https://127.0.0.1:{}",
                callback_url.port().unwrap()
            ));
        }
        f.calls.jmap.lock().unwrap().clear();
        f.calls.workloads.lock().unwrap().clear();
        let before = f.snapshot().await;
        let mut child = f.start(&bad);
        let mut client = f.client(&mut child).await;
        client.describe(wire::DescribeRequest {}).await.unwrap();
        assert!(
            client
                .invoke(invoke(
                    1,
                    "domains.create",
                    json!({"domain":"bad-callback.example.test"})
                ))
                .await
                .is_err()
        );
        // More than one startup backoff period, with the ordinary authority server already active.
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(before == f.snapshot().await);
        assert!(f.calls.jmap.lock().unwrap().is_empty());
        assert!(f.calls.workloads.lock().unwrap().is_empty());
        drop(client);
        shutdown(&mut child).await;
        drop(child);
    }
}
