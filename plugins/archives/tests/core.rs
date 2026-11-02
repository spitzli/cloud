#![cfg(feature = "local-test")]
//! Uses existing owned Core fixture without changing its catalog, gateway or migrations.
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use std::sync::Arc;
use webdock_identity::{CookiePolicy, IdentityService};
use webdock_plugin_host::{
    AdmissionInput, FileBindings, HostService,
    storage::{StorageCaller, StorageGateway},
};
use webdock_plugin_protocol::{Manifest, storage::StoragePlan};
use webdock_plugin_runtime::{CorePeer, PeerIdentity, RemoteAuthority, Runtime, UnixSocket, wire};
#[allow(
    dead_code,
    reason = "shared existing fixture includes unused jobs helpers"
)]
#[path = "../../../crates/plugin-host/tests/support/jobs_fixture.rs"]
mod fixture;
fn peer() -> PeerIdentity {
    PeerIdentity::Unix {
        uid: rustix::process::geteuid().as_raw(),
    }
}
async fn sql(db: &impl ConnectionTrait, text: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        text,
        values,
    ))
    .await
    .unwrap();
}
struct Task(tokio::task::JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn actor(
    f: &fixture::JobFixture,
    subject: &str,
    session: &str,
) -> webdock_identity::Principal {
    let tx = f.auth.begin().await.unwrap();
    let actor = f
        .identity
        .authenticate_session_in(&tx, subject, session)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    actor
}
async fn call(
    f: &fixture::JobFixture,
    client: &mut wire::plugin_runtime_client::PluginRuntimeClient<tonic::transport::Channel>,
    action: &str,
    input: Value,
) -> (webdock_plugin_host::AdmittedInvocation, Value) {
    let member = actor(f, "member", "member-session").await;
    let admitted=f.host.admit_with_authority(&member,serde_json::from_value::<AdmissionInput>(json!({"pluginID":"archives","projectID":"124","action":action,"input":input,"idempotencyKey":uuid::Uuid::new_v4().to_string()})).unwrap(),true,&webdock_browser_session::StoredAuthority::local_session(&member)).await.unwrap();
    let out = client
        .invoke(wire::InvokeRequest {
            invocation_token: admitted.token.clone(),
            action: action.into(),
            input_json: serde_json::to_vec(&input).unwrap(),
            configuration_json: b"{}".to_vec(),
        })
        .await
        .unwrap()
        .into_inner();
    let out = serde_json::from_slice(&out.output_json).unwrap();
    (admitted, out)
}
struct CurrentAuthority {
    host: Arc<HostService>,
    actor: webdock_identity::Principal,
    runtime: wire::plugin_runtime_client::PluginRuntimeClient<tonic::transport::Channel>,
    id: String,
}
#[async_trait::async_trait]
impl webdock_plugin_host::storage::StorageAuthority for CurrentAuthority {
    async fn check(&self) -> Result<(), webdock_plugin_protocol::PluginError> {
        let caller = StorageCaller::browser(
            &self.actor,
            webdock_browser_session::StoredAuthority::local_session(&self.actor),
            true,
        );
        let access = self.host.storage_access(&self.id, "GET", &caller).await?;
        self.runtime
            .clone()
            .authorize_storage(wire::StorageAuthorizationRequest {
                authorization_json: serde_json::to_vec(&access.authorization)
                    .map_err(|_| webdock_plugin_protocol::PluginError::denied())?,
            })
            .await
            .map_err(|_| webdock_plugin_protocol::PluginError::denied())?;
        Ok(())
    }
}
#[tokio::test]
#[ignore = "owns an isolated Core PostgreSQL fixture and real native producer/runtime on existing MinIO"]
async fn core_jobs_reservations_and_original_download_authority_remain_canonical() {
    webdock_archives::initialize_crypto();
    let mut f = fixture::JobFixture::start_with_connections(8).await;
    let mut params = rcgen::CertificateParams::new(vec![]).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let ca = params.self_signed(&ca_key).unwrap();
    let issuer = rcgen::Issuer::new(params, ca_key);
    let leaf = |server| {
        let mut p = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
        p.extended_key_usages = vec![if server {
            rcgen::ExtendedKeyUsagePurpose::ServerAuth
        } else {
            rcgen::ExtendedKeyUsagePurpose::ClientAuth
        }];
        let key = rcgen::KeyPair::generate().unwrap();
        (p.signed_by(&key, &issuer).unwrap(), key)
    };
    let (server_cert, server_key) = leaf(true);
    let (client_cert, client_key) = leaf(false);

    let binding: Value = serde_json::from_slice(
        &std::fs::read(std::env::var("WEBDOCK_ARCHIVES_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let manifest: Value = serde_json::from_slice(include_bytes!("../manifest.json")).unwrap();
    use sha2::{Digest, Sha256};
    let raw = manifest.to_string();
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES('archives','external','enabled','enabled','0.3.0','ready',1)",vec![]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES('archives','0.3.0',$1,$2,$3,'https://registry.invalid','fixture')",vec![raw.clone().into(),hex::encode(Sha256::digest(raw)).into(),manifest["image"].as_str().unwrap().into()]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES('archives',$1,'{}')",vec![manifest["capabilities"].clone().into()]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) VALUES('archives',1,'0.3.0',$1,'ready',0,$2,'fixtures','fixture-plugin')",vec![manifest["image"].as_str().unwrap().into(),"a".repeat(64).into()]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_feature_policy(plugin_id,feature_id,mode,updated_by) VALUES('archives','exports','everyone','operator')",vec![]).await;
    let path = f.directory.path().join("bindings.json");
    let mut bindings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    bindings["runtimes"]["archives"] = bindings["runtimes"]["job-fixture"].clone();
    bindings["runtimes"]["archives"]["peers"].as_array_mut().unwrap().push(json!({"kind":"mutualTls","certificateSha256":hex::encode(sha2::Sha256::digest(client_cert.der()))}));
    bindings["runtimes"]["archives"]["storage"] = json!({"124":{"archives":{"origin":binding["backend"]["config"]["endpoint"],"addresses":["127.0.0.1"],"ca":null,"pathPrefix":format!("/{}/",binding["backend"]["config"]["bucket"].as_str().unwrap())}}});
    std::fs::write(&path, bindings.to_string()).unwrap();
    f.host = Arc::new(
        Arc::try_unwrap(f.host)
            .ok()
            .unwrap()
            .with_storage_gateway(Arc::new(
                StorageGateway::loopback_test(f.directory.path().into()).unwrap(),
            )),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!(
        "https://localhost:{}",
        listener.local_addr().unwrap().port()
    );
    let exports = f
        .host
        .clone()
        .export_service(Arc::new(webdock_plugin_host::UnconfiguredExportSources));
    let service = f.host.clone().rpc();
    let tls = tonic::transport::ServerTlsConfig::new()
        .identity(tonic::transport::Identity::from_pem(
            server_cert.pem(),
            server_key.serialize_pem(),
        ))
        .client_ca_root(tonic::transport::Certificate::from_pem(ca.pem()))
        .client_auth_optional(false);
    let _host = Task(tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(tls)
            .unwrap()
            .add_service(service)
            .add_service(exports)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    }));
    use std::os::unix::fs::PermissionsExt;
    let cert_path = f.directory.path().join("client.crt");
    let key_path = f.directory.path().join("client.key");
    let ca_path = f.directory.path().join("ca.crt");
    for (path, pem) in [
        (&cert_path, client_cert.pem()),
        (&key_path, client_key.serialize_pem()),
        (&ca_path, ca.pem()),
    ] {
        std::fs::write(path, pem).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let transport = webdock_plugin_kit::transport::RuntimeTransport {
        listen: Some(f.directory.path().join("runtime.sock")),
        core_tls: Some(webdock_plugin_kit::transport::CoreTls {
            origin,
            domain: "localhost".into(),
            certificate: cert_path,
            private_key: key_path,
            authority: ca_path,
            server_certificate_sha256: hex::encode(sha2::Sha256::digest(server_cert.der())),
        }),
        ..Default::default()
    };
    let host = transport.core_client().await.unwrap();
    host.authorize_runtime("archives", 1).await.unwrap();
    let backend = Arc::new(
        webdock_archives::backend::Backend::local_test(
            serde_json::from_value(binding["backend"].clone()).unwrap(),
        )
        .await
        .unwrap(),
    );
    let engine = Arc::new(
        webdock_archives::Engine::connect(
            binding["database"].as_str().unwrap(),
            None,
            backend,
            f.directory.path().into(),
            vec![],
        )
        .await
        .unwrap(),
    );
    let handler = Arc::new(webdock_archives::Handler {
        engine: engine.clone(),
        host: host.clone(),
    });
    let runtime = Runtime::new(
        Manifest::parse(include_bytes!("../manifest.json")).unwrap(),
        1,
        ed25519_dalek::SigningKey::from_bytes(&[19; 32]).verifying_key(),
        Arc::new(
            RemoteAuthority::new(
                host,
                "archives".into(),
                1,
                vec![CorePeer::Unix {
                    uid: rustix::process::geteuid().as_raw(),
                }],
            )
            .unwrap(),
        ),
        handler.clone(),
    )
    .unwrap()
    .with_jobs(handler);
    let runtime_path = f.directory.path().join("runtime.sock");
    let socket = UnixSocket::bind(&runtime_path).unwrap();
    let _runtime = Task(tokio::spawn(async move {
        runtime
            .serve_unix(socket, std::future::pending())
            .await
            .unwrap();
    }));
    let mut client = webdock_plugin_runtime::connect_unix(&runtime_path)
        .await
        .unwrap();
    let id = uuid::Uuid::new_v4();
    let (created,out)=call(&f,&mut client,"archives.create",json!({"request_id":id,"kind":"diagnostics","retention_seconds":3600,"diagnostics":{"version":1,"state":"finished","builds":[],"checks":[]}})).await;
    let h = f.host.authenticate(peer(), &created.token).await.unwrap();
    f.host.record_resource(&h, &out).await.unwrap();
    f.host.revoke_issued(&created.token).await.unwrap();
    let (_, queued) = call(
        &f,
        &mut client,
        "archives.generate",
        json!({"resource_id":id}),
    )
    .await;
    let lease = f.host.claim_job(Some("archives")).await.unwrap().unwrap();
    assert_eq!(queued["id"], lease.id());
    let result = client
        .run_job(wire::RunJobRequest {
            invocation_token: lease.invocation().token.clone(),
            name: "archives.produce".into(),
            input_json: serde_json::to_vec(&json!({"resource_id":id})).unwrap(),
            configuration_json: b"{}".to_vec(),
        })
        .await
        .unwrap()
        .into_inner();
    // Verified S3 bytes alone must not unlock a public download before Core completes the job.
    let member = actor(&f, "member", "member-session").await;
    let pending=f.host.admit_with_authority(&member,serde_json::from_value::<AdmissionInput>(json!({"pluginID":"archives","projectID":"124","action":"archives.download","input":{"resource_id":id},"idempotencyKey":uuid::Uuid::new_v4().to_string()})).unwrap(),true,&webdock_browser_session::StoredAuthority::local_session(&member)).await.unwrap();
    assert!(
        client
            .invoke(wire::InvokeRequest {
                invocation_token: pending.token,
                action: "archives.download".into(),
                input_json: serde_json::to_vec(&json!({"resource_id":id})).unwrap(),
                configuration_json: b"{}".to_vec()
            })
            .await
            .is_err()
    );
    f.host
        .complete_job(
            &lease,
            Ok(serde_json::from_slice(&result.output_json).unwrap()),
        )
        .await
        .unwrap();
    let (download, out) = call(
        &f,
        &mut client,
        "archives.download",
        json!({"resource_id":id}),
    )
    .await;
    let p: StoragePlan = serde_json::from_value(out).unwrap();
    let h = f.host.authenticate(peer(), &download.token).await.unwrap();
    let descriptor = f
        .host
        .retain_storage_plan_with_api(&h, p, "c".repeat(64), None)
        .await
        .unwrap();
    f.host.revoke_issued(&download.token).await.unwrap();
    let member = actor(&f, "member", "member-session").await;
    let caller = StorageCaller::browser(
        &member,
        webdock_browser_session::StoredAuthority::local_session(&member),
        true,
    );
    let access = f
        .host
        .storage_access(&descriptor.id, "GET", &caller)
        .await
        .unwrap();
    client
        .authorize_storage(wire::StorageAuthorizationRequest {
            authorization_json: serde_json::to_vec(&access.authorization).unwrap(),
        })
        .await
        .unwrap();
    let foreign = actor(&f, "operator", "session").await;
    assert!(
        f.host
            .storage_access(
                &descriptor.id,
                "GET",
                &StorageCaller::browser(
                    &foreign,
                    webdock_browser_session::StoredAuthority::local_session(&foreign),
                    true
                )
            )
            .await
            .is_err()
    );
    let authority = Arc::new(CurrentAuthority {
        host: f.host.clone(),
        actor: actor(&f, "member", "member-session").await,
        runtime: client.clone(),
        id: descriptor.id.clone(),
    });
    let staged = f
        .host
        .storage_gateway()
        .unwrap()
        .download(
            &access.plan,
            f.host
                .storage_gateway()
                .unwrap()
                .target(&access.plan, access.binding.clone())
                .unwrap(),
            authority,
        )
        .await
        .unwrap();
    let mut pending_delivery = staged.into_stream();
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_feature_policy SET mode='selected' WHERE plugin_id='archives'",
        vec![],
    )
    .await;
    assert!(
        f.host
            .storage_access(&descriptor.id, "GET", &caller)
            .await
            .is_err()
    );
    assert!(
        pending_delivery.next_chunk().await.is_err(),
        "entitlement is rechecked after staging and before delivery"
    );
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_feature_policy SET mode='everyone' WHERE plugin_id='archives'",
        vec![],
    )
    .await;
    assert!(
        f.host
            .storage_access(&descriptor.id, "GET", &caller)
            .await
            .is_ok()
    );
    // A reader with no database-export entitlement must not gain bytes from a
    // historical unqualified PG receipt, including a previously issued ticket.
    sql(
        &f.owner,
        "UPDATE webdock_auth.member SET role='member' WHERE id='member-binding'",
        vec![],
    )
    .await;
    let reader = actor(&f, "member", "member-session").await;
    let signer = webdock_identity::RlsSigner::new(
        "owned-plugin-jobs-rls-secret-32bytes",
        "webdock_auth_runtime",
    )
    .unwrap();
    let business = f.database.role("webdock_auth_runtime").await;
    let tx = signer.begin(&business, &reader).await.unwrap();
    assert!(
        !tx.query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT webdock_rls.customer_permission('123','org:databases:manage') AS allowed"
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<bool>("", "allowed")
        .unwrap()
    );
    tx.commit().await.unwrap();
    let (metadata, task) =
        tokio_postgres::connect(binding["database"].as_str().unwrap(), tokio_postgres::NoTls)
            .await
            .unwrap();
    tokio::spawn(async move {
        let _ = task.await;
    });
    let row = metadata
        .query_one("SELECT body FROM archives WHERE id=$1", &[&id])
        .await
        .unwrap();
    let mut legacy: Value = serde_json::from_str(row.get(0)).unwrap();
    legacy["kind"] = json!("postgres");
    legacy["source_binding"] = json!("a".repeat(64));
    metadata
        .execute(
            "UPDATE archives SET body=$2 WHERE id=$1",
            &[&id, &legacy.to_string()],
        )
        .await
        .unwrap();
    assert!(
        client
            .authorize_storage(wire::StorageAuthorizationRequest {
                authorization_json: serde_json::to_vec(&access.authorization).unwrap()
            })
            .await
            .is_err(),
        "old ticket must fail on legacy receipt before bytes"
    );
    for action in ["archives.download", "archives.create"] {
        let input = if action == "archives.download" {
            json!({"resource_id":id})
        } else {
            json!({"request_id":uuid::Uuid::new_v4(),"kind":"postgres","retention_seconds":3600})
        };
        let admitted=f.host.admit_with_authority(&reader,serde_json::from_value::<AdmissionInput>(json!({"pluginID":"archives","projectID":"124","action":action,"input":input,"idempotencyKey":uuid::Uuid::new_v4().to_string()})).unwrap(),true,&webdock_browser_session::StoredAuthority::local_session(&reader)).await;
        if action == "archives.download" {
            assert!(
                admitted.is_ok(),
                "reader still has ordinary resource read authority"
            );
        }
        if let Ok(admitted) = admitted {
            assert!(
                client
                    .invoke(wire::InvokeRequest {
                        invocation_token: admitted.token,
                        action: action.into(),
                        input_json: serde_json::to_vec(&input).unwrap(),
                        configuration_json: b"{}".to_vec()
                    })
                    .await
                    .is_err()
            );
        }
    }
    sql(
        &f.owner,
        "UPDATE webdock_auth.member SET role='owner' WHERE id='member-binding'",
        vec![],
    )
    .await;
    call(
        &f,
        &mut client,
        "archives.delete",
        json!({"resource_id":id}),
    )
    .await;
    assert_eq!(
        engine.status("123", "124", id).await.unwrap()["state"],
        "deleted"
    );
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.member WHERE id='member-binding'",
        vec![],
    )
    .await;
    assert!(
        f.host
            .storage_access(&descriptor.id, "GET", &caller)
            .await
            .is_err()
    );
    // Producer denial independently revokes the already minted opaque transfer.
    engine.delete("123", "124", id).await.unwrap();
    assert!(
        client
            .authorize_storage(wire::StorageAuthorizationRequest {
                authorization_json: serde_json::to_vec(&access.authorization).unwrap()
            })
            .await
            .is_err()
    );
}
