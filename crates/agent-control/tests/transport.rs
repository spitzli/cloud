use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tonic::transport::{Endpoint, Server};
use webdock_agent_control::{ControlPlane, LocalBinding};
use webdock_agent_protocol::{self as wire, agent_control_client::AgentControlClient};
struct NegotiationBackend(std::sync::Mutex<Vec<u32>>);
#[tonic::async_trait]
impl webdock_agent_control::PluginRuntimeBackend for NegotiationBackend {
    fn supported_spec_version(&self) -> u32 {
        2
    }
    async fn poll(
        &self,
        _: &webdock_hosting::AgentIdentity,
        max: u32,
    ) -> Result<Option<wire::plugin::PluginRuntimeClaim>, tonic::Status> {
        self.0.lock().unwrap().push(max);
        Ok(None)
    }
    async fn guard(
        &self,
        _: &webdock_hosting::AgentIdentity,
        _: &wire::plugin::PluginRuntimeSpec,
        _: &wire::plugin::PluginRuntimeLease,
    ) -> Result<bool, tonic::Status> {
        Ok(false)
    }
    async fn checkpoint(
        &self,
        _: &webdock_hosting::AgentIdentity,
        _: &wire::plugin::PluginRuntimeSpec,
        _: &wire::plugin::PluginRuntimeLease,
        _: &wire::plugin::PluginRuntimeCheckpoint,
    ) -> Result<bool, tonic::Status> {
        Ok(false)
    }
    async fn complete(
        &self,
        _: &webdock_hosting::AgentIdentity,
        _: &wire::plugin::PluginRuntimeSpec,
        _: &wire::plugin::PluginRuntimeLease,
        _: &wire::plugin::PluginRuntimeCompletion,
    ) -> Result<bool, tonic::Status> {
        Ok(false)
    }
}
async fn db(key: &str) -> DatabaseConnection {
    let raw = std::env::var(key).unwrap();
    let url = url::Url::parse(&raw).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    Database::connect(raw).await.unwrap()
}
async fn execute(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}
#[tokio::test]
#[ignore = "requires owned retained-schema local fixture; only unique test enrollment is mutated"]
async fn real_transport_enrollment_generation_revocation_and_connectivity_heartbeat() {
    let owner = db("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
    let auth = db("AUTH_DATABASE_URL").await;
    let ids = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &std::env::var("AUTH_DATABASE_URL").unwrap(),
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let cluster = ids.allocate().await.unwrap().to_string();
    let installation = format!("rpc-{}", &cluster[cluster.len() - 12..]);
    let hash = hex::encode(Sha256::digest(format!("local-fixture-{cluster}")));
    execute(&owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,generation,verified,capacity,runtime_kind,runtime_installation) VALUES($1,$2,'fixture','local','owned isolated RPC fixture',1,true,'{}','docker',$2)",vec![cluster.clone().into(),installation.clone().into()]).await;
    execute(&owner,"INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_sequence,last_seen,observation) VALUES($1,$2,1,0,NULL,'{}')",vec![cluster.clone().into(),hash.clone().into()]).await;
    let uid = rustix::process::geteuid().as_raw();
    execute(&owner,"INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid) VALUES($1,'local-agent',1,$2,$3,'unix',$4)",vec![installation.clone().into(),cluster.clone().into(),hash.into(),i64::from(uid).into()]).await;
    assert!(
        auth.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.agent_transport_enrollment SET revoked=true WHERE installation=$1",
            [installation.clone().into()]
        ))
        .await
        .is_err(),
        "runtime must never revoke or replace its own enrollment"
    );
    let hosting = Arc::new(
        webdock_hosting::HostingService::new(
            db("DATABASE_URL").await,
            auth.clone(),
            db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            std::env::var("BETTER_AUTH_SECRET").unwrap(),
            ids,
        )
        .await
        .unwrap(),
    );
    let control = ControlPlane::new(
        auth.clone(),
        hosting.clone(),
        Some(LocalBinding {
            installation: installation.clone(),
            agent_id: "local-agent".into(),
            uid,
        }),
    )
    .await
    .unwrap();
    let negotiated_backend = Arc::new(NegotiationBackend(std::sync::Mutex::new(Vec::new())));
    let versioned_control = control
        .clone()
        .with_plugin_runtime(negotiated_backend.clone());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("control.sock");
    let socket = tokio::net::UnixListener::bind(&path).unwrap();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        Server::builder()
            .add_service(control.clone().database_service())
            .add_service(control.clone().plugin_runtime_service())
            .add_service(control.service())
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::UnixListenerStream::new(socket),
                async {
                    let _ = shutdown.await;
                },
            )
            .await
            .unwrap();
    });
    let channel = Endpoint::from_shared(format!("unix://{}", path.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut database_client =
        wire::database_control_client::DatabaseControlClient::new(channel.clone());
    let mut plugin_client =
        wire::plugin_runtime_control_client::PluginRuntimeControlClient::new(channel.clone());
    let mut client = AgentControlClient::new(channel);
    let identity = wire::AgentIdentity {
        installation: installation.clone(),
        agent_id: "local-agent".into(),
        generation: 1,
    };
    let hello = wire::Hello {
        identity: Some(identity.clone()),
        protocol_version: 1,
        certificate_sha256: String::new(),
        capabilities_json: b"{}".to_vec(),
    };
    assert!(
        client
            .register(hello.clone())
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    assert_eq!(
        database_client
            .poll_database(wire::PollRequest {
                identity: Some(identity.clone())
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::FailedPrecondition,
        "database execution must default deny without its domain authority"
    );
    assert_eq!(
        plugin_client
            .poll_plugin_runtime(wire::PluginRuntimePollRequest {
                identity: Some(identity.clone()),
                max_spec_version: 0,
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unimplemented,
        "unconfigured plugin deployment is an optional unavailable domain"
    );
    let versioned_path = directory.path().join("plugin-control.sock");
    let versioned_socket = tokio::net::UnixListener::bind(&versioned_path).unwrap();
    let (versioned_stop, versioned_shutdown) = tokio::sync::oneshot::channel();
    let versioned_server = tokio::spawn(async move {
        Server::builder()
            .add_service(versioned_control.plugin_runtime_service())
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::UnixListenerStream::new(versioned_socket),
                async {
                    let _ = versioned_shutdown.await;
                },
            )
            .await
            .unwrap();
    });
    let versioned_channel = Endpoint::from_shared(format!("unix://{}", versioned_path.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut versioned_client =
        wire::plugin_runtime_control_client::PluginRuntimeControlClient::new(versioned_channel);
    for max_spec_version in [0, 1, 2] {
        let reply = versioned_client
            .poll_plugin_runtime(wire::PluginRuntimePollRequest {
                identity: Some(identity.clone()),
                max_spec_version,
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(reply.supported_spec_version, 2);
        assert!(reply.command.is_none());
    }
    assert_eq!(
        *negotiated_backend.0.lock().unwrap(),
        vec![1, 1, 2],
        "old request max remains1 regardless of any stored capability observation"
    );
    assert_eq!(
        versioned_client
            .poll_plugin_runtime(wire::PluginRuntimePollRequest {
                identity: Some(identity.clone()),
                max_spec_version: 3
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );
    assert_eq!(*negotiated_backend.0.lock().unwrap(), vec![1, 1, 2]);
    versioned_stop.send(()).unwrap();
    versioned_server.await.unwrap();
    let mut forged = hello.clone();
    forged.certificate_sha256 = "a".repeat(64);
    assert!(client.register(forged).await.is_err());
    let mut wrong = hello.clone();
    wrong.identity.as_mut().unwrap().agent_id = "other-agent".into();
    assert!(client.register(wrong).await.is_err());
    let heartbeat = wire::HeartbeatRequest {
        identity: Some(identity.clone()),
        sequence: 1,
        observed_at: chrono::Utc::now().timestamp() as u64,
        observation_json: vec![],
    };
    assert!(
        client
            .heartbeat(heartbeat.clone())
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    assert!(client.heartbeat(heartbeat).await.is_err());
    let last_seen=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT last_seen IS NULL AS unchanged FROM webdock_auth.hosting_agent WHERE cluster_id=$1",[cluster.clone().into()])).await.unwrap().unwrap().try_get::<bool>("","unchanged").unwrap();
    assert!(
        last_seen,
        "connectivity heartbeat must not refresh capacity observations"
    );
    execute(
        &owner,
        "UPDATE webdock_auth.hosting_agent SET generation=2 WHERE cluster_id=$1",
        vec![cluster.clone().into()],
    )
    .await;
    assert!(client.register(hello.clone()).await.is_err());
    execute(
        &owner,
        "UPDATE webdock_auth.hosting_agent SET generation=1 WHERE cluster_id=$1",
        vec![cluster.clone().into()],
    )
    .await;
    execute(
        &owner,
        "UPDATE webdock_auth.agent_transport_enrollment SET revoked=true WHERE installation=$1",
        vec![installation.clone().into()],
    )
    .await;
    assert!(
        client.register(hello).await.is_err(),
        "revocation must apply on an existing channel"
    );
    assert_eq!(
        database_client
            .poll_database(wire::PollRequest {
                identity: Some(identity.clone())
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied,
        "database RPC rechecks revoked transport on the same channel"
    );
    assert_eq!(
        plugin_client
            .poll_plugin_runtime(wire::PluginRuntimePollRequest {
                identity: Some(identity.clone()),
                max_spec_version: 0,
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(
        plugin_client
            .guard_plugin_runtime(wire::PluginRuntimeGuardRequest {
                identity: Some(identity.clone()),
                spec_json: vec![],
                lease_json: vec![],
                request_sha256: String::new()
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(
        plugin_client
            .checkpoint_plugin_runtime(wire::PluginRuntimeCheckpointRequest {
                identity: Some(identity.clone()),
                spec_json: vec![],
                lease_json: vec![],
                request_sha256: String::new(),
                checkpoint_json: vec![]
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(
        plugin_client
            .complete_plugin_runtime(wire::PluginRuntimeReceipt {
                identity: Some(identity.clone()),
                spec_json: vec![],
                lease_json: vec![],
                request_sha256: String::new(),
                completion_json: vec![]
            })
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied,
        "historical callback delivery still requires current transport enrollment"
    );
    stop.send(()).unwrap();
    server.await.unwrap();
    // Switch only this owned fixture enrollment to a verified remote client leaf.
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let issuer = rcgen::Issuer::new(ca_params, ca_key);
    let certificate = |server: bool| {
        let mut params = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
        params.extended_key_usages = vec![if server {
            rcgen::ExtendedKeyUsagePurpose::ServerAuth
        } else {
            rcgen::ExtendedKeyUsagePurpose::ClientAuth
        }];
        let key = rcgen::KeyPair::generate().unwrap();
        (params.signed_by(&key, &issuer).unwrap(), key)
    };
    let (server_cert, server_key) = certificate(true);
    let (client_cert, client_key) = certificate(false);
    let fingerprint = hex::encode(Sha256::digest(client_cert.der()));
    execute(&owner,"UPDATE webdock_auth.agent_transport_enrollment SET transport='mtls',unix_uid=NULL,certificate_sha256=$2,revoked=false WHERE installation=$1",vec![installation.clone().into(),fingerprint.clone().into()]).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let control = ControlPlane::new(auth, hosting, None).await.unwrap();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let ca_pem = ca.pem();
    let server_ca = ca_pem.clone();
    let server = tokio::spawn(async move {
        control
            .serve_mtls(
                listener,
                webdock_plugin_runtime::MutualTls {
                    identity: tonic::transport::Identity::from_pem(
                        server_cert.pem(),
                        server_key.serialize_pem(),
                    ),
                    peer_ca: tonic::transport::Certificate::from_pem(server_ca),
                },
                async {
                    let _ = shutdown.await;
                },
            )
            .await
            .unwrap();
    });
    let endpoint = Endpoint::from_shared(format!("https://{address}"))
        .unwrap()
        .tls_config(
            tonic::transport::ClientTlsConfig::new()
                .domain_name("localhost")
                .ca_certificate(tonic::transport::Certificate::from_pem(ca_pem))
                .identity(tonic::transport::Identity::from_pem(
                    client_cert.pem(),
                    client_key.serialize_pem(),
                )),
        )
        .unwrap();
    let mut client = AgentControlClient::new(endpoint.connect().await.unwrap());
    let hello = wire::Hello {
        identity: Some(identity),
        protocol_version: 1,
        certificate_sha256: fingerprint,
        capabilities_json: b"{}".to_vec(),
    };
    assert!(
        client
            .register(hello.clone())
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    let mut wrong = hello.clone();
    wrong.certificate_sha256 = "b".repeat(64);
    assert!(client.register(wrong).await.is_err());
    execute(
        &owner,
        "UPDATE webdock_auth.agent_transport_enrollment SET revoked=true WHERE installation=$1",
        vec![installation.clone().into()],
    )
    .await;
    assert!(
        client.register(hello).await.is_err(),
        "remote revocation must affect an existing TLS channel"
    );
    stop.send(()).unwrap();
    server.await.unwrap();
    execute(
        &owner,
        "DELETE FROM webdock_auth.agent_transport_enrollment WHERE installation=$1",
        vec![installation.into()],
    )
    .await;
    execute(
        &owner,
        "DELETE FROM webdock_auth.hosting_agent WHERE cluster_id=$1",
        vec![cluster.clone().into()],
    )
    .await;
    execute(
        &owner,
        "DELETE FROM webdock_auth.hosting_cluster WHERE id=$1",
        vec![cluster.into()],
    )
    .await;
}
