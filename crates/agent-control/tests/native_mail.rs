//! Actual restricted Core → enrolled daemon → isolated Community Edition lifecycle.
use base64::{Engine, engine::general_purpose::STANDARD};
use bollard::{Docker, query_parameters::*};
use futures_util::FutureExt;
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TransactionTrait,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tonic::{Request, Response, Status};
use webdock_agent::{
    config::{AgentConfig, ControlEndpoint, RuntimeConfiguration},
    management::Daemon,
};
use webdock_agent_control::{ControlPlane, LocalBinding};
use webdock_agent_protocol::agent_control_client::AgentControlClient;
use webdock_agent_protocol::{
    self as wire,
    mail_control_server::{MailControl, MailControlServer},
};
use webdock_browser_session::StoredAuthority;
use webdock_identity::{CookiePolicy, IdentityService};
use webdock_mail::{
    MailDependencies,
    incoming::{
        IncomingConfig, NativeIncoming, ServiceCommand, ServiceState,
        agent::{IncomingAgent, MailAgentConfig},
    },
};

const KEY: &str = "owned-native-mail-control-fixture-key-atleast32";
#[path = "support/native_mail_browser.rs"]
mod browser;
#[path = "support/native_mail_configuration.rs"]
mod configuration;
#[derive(Clone, Copy)]
enum Scenario {
    Lifecycle,
    Browser,
    BrowserConfiguration,
}
struct LostAck {
    core: ControlPlane,
    checkpoint: AtomicBool,
    complete: AtomicBool,
    checkpoints: Mutex<Vec<wire::MailCheckpointRequest>>,
    receipts: Mutex<Vec<wire::MailReceipt>>,
}
struct MailService(Arc<LostAck>);
#[tonic::async_trait]
impl MailControl for MailService {
    async fn poll_mail(
        &self,
        r: Request<wire::PollRequest>,
    ) -> Result<Response<wire::MailDispatch>, Status> {
        self.0.core.poll_mail(r).await
    }
    async fn guard_mail(
        &self,
        r: Request<wire::MailGuardRequest>,
    ) -> Result<Response<wire::GuardReply>, Status> {
        self.0.core.guard_mail(r).await
    }
    async fn checkpoint_mail(
        &self,
        r: Request<wire::MailCheckpointRequest>,
    ) -> Result<Response<wire::Ack>, Status> {
        let captured = r.get_ref().clone();
        let result = self.0.core.checkpoint_mail(r).await?;
        self.0.checkpoints.lock().unwrap().push(captured);
        if self.0.checkpoint.swap(false, Ordering::SeqCst) {
            return Err(Status::unavailable("owned test dropped checkpoint ACK"));
        }
        Ok(result)
    }
    async fn complete_mail(
        &self,
        r: Request<wire::MailReceipt>,
    ) -> Result<Response<wire::Ack>, Status> {
        let captured = r.get_ref().clone();
        let result = self.0.core.complete_mail(r).await?;
        self.0.receipts.lock().unwrap().push(captured);
        if self.0.complete.swap(false, Ordering::SeqCst) {
            return Err(Status::unavailable("owned test dropped completion ACK"));
        }
        Ok(result)
    }
}
async fn sql(db: &impl ConnectionTrait, query: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        query,
        values,
    ))
    .await
    .unwrap();
}
async fn count(db: &impl ConnectionTrait, query: &str) -> i64 {
    db.query_one_raw(Statement::from_string(DbBackend::Postgres, query))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "count")
        .unwrap()
}
async fn role(key: &str, name: &str, host: &url::Url) -> DatabaseConnection {
    let mut url = url::Url::parse(&std::env::var(key).unwrap()).unwrap();
    assert_eq!(url.host_str(), host.host_str());
    assert_eq!(url.port(), host.port());
    url.set_path(&format!("/{name}"));
    Database::connect(url.as_str()).await.unwrap()
}
fn private(path: &std::path::Path, value: &impl serde::Serialize) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(&serde_json::to_vec(value).unwrap()).unwrap();
    file.sync_all().unwrap();
}
async fn disabled_hosting_policy(
    owner: &DatabaseConnection,
    path: &std::path::Path,
    installation: &str,
    image: &str,
) {
    use prost::Message;
    let command:webdock_supervisor::Command=serde_json::from_value(json!({
        "operationId":"901","generation":1,"leaseUntil":chrono::Utc::now().timestamp()+120,
        "authorization":"owned-fixture-reference","action":"stop",
        "spec":{"version":1,"installation":installation,"projectId":"123","resourceId":"456","revision":1,
            "image":image,"arguments":[],"cpuMillicores":100,"memoryBytes":33554432,"ephemeralBytes":16777216,
            "replicas":0,"port":8080,"healthPath":null,"volume":null}
    })).unwrap();
    let identity = wire::AgentIdentity {
        installation: installation.into(),
        agent_id: "mail-agent".into(),
        generation: 1,
    };
    let receipt = wire::Receipt {
        identity: Some(identity.clone()),
        operation_id: "901".into(),
        generation: 1,
        request_sha256: command.fingerprint().unwrap(),
        outcome: wire::Outcome::Rejected as i32,
        error_code: "owned-fixture-terminal-failure".into(),
        result: None,
    };
    let receipt_hash = hex::encode(Sha256::digest(receipt.encode_to_vec()));
    sql(owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('hosting','builtin','enabled','enabled','owned-feature-policy-fixture')",vec![]).await;
    sql(owner,"INSERT INTO webdock_auth.hosting_operation(id,subject,customer_id,cluster_id,action,idempotency_key,payload_hash,status,generation,actor_session) VALUES('901','1001','123','200','apps.stop','owned-policy','owned-policy','failed',1,'operator-session')",vec![]).await;
    sql(owner,"INSERT INTO webdock_auth.agent_runtime_dispatch(operation_id,generation,installation,agent_id,agent_generation,request_sha256,command,receipt_sha256) VALUES('901',1,$1,'mail-agent',1,$2,$3,$4)",vec![installation.into(),command.fingerprint().unwrap().into(),serde_json::to_value(&command).unwrap().into(),receipt_hash.into()]).await;
    let channel = tonic::transport::Endpoint::from_shared(format!("unix://{}", path.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut client = AgentControlClient::new(channel);
    assert!(
        client
            .complete(receipt.clone())
            .await
            .unwrap()
            .into_inner()
            .accepted,
        "accepted exact receipts remain deliverable after feature disable"
    );
    sql(owner,"UPDATE webdock_auth.agent_runtime_dispatch SET receipt_sha256=NULL WHERE operation_id='901'",vec![]).await;
    assert!(
        client
            .poll(wire::PollRequest {
                identity: Some(identity.clone())
            })
            .await
            .unwrap()
            .into_inner()
            .command
            .is_none()
    );
    let guard = wire::GuardRequest {
        identity: Some(identity),
        operation_id: "901".into(),
        generation: 1,
        lease_until: command.lease_until,
        authorization: command.authorization.clone(),
        intent: wire::Intent::Stop as i32,
        request_sha256: command.fingerprint().unwrap(),
    };
    assert_eq!(
        client.guard(guard).await.unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    assert_eq!(
        client.complete(receipt).await.unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    sql(
        owner,
        "DELETE FROM webdock_auth.hosting_operation WHERE id='901'",
        vec![],
    )
    .await;
}
fn own(labels: Option<&std::collections::HashMap<String, String>>, installation: &str) {
    let labels = labels.expect("owned fixture labels");
    assert_eq!(
        labels.get("webdock.mail.installation").map(String::as_str),
        Some(installation)
    );
    assert_eq!(
        labels.get("webdock.mail.customer").map(String::as_str),
        Some("123")
    );
    assert_eq!(
        labels.get("webdock.mail.cluster").map(String::as_str),
        Some("200")
    );
}
async fn cleanup(docker: &Docker, installation: &str) {
    let base = format!("wd-mail-{installation}-123");
    for name in [&base, &format!("{base}-init")] {
        match docker
            .inspect_container(name, None::<InspectContainerOptions>)
            .await
        {
            Ok(container) => {
                own(
                    container.config.as_ref().and_then(|c| c.labels.as_ref()),
                    installation,
                );
                docker
                    .remove_container(
                        name,
                        Some(RemoveContainerOptionsBuilder::default().force(true).build()),
                    )
                    .await
                    .unwrap();
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {}
            Err(error) => panic!("owned container inspection failed: {error}"),
        }
    }
    let volume = format!("{base}-data");
    match docker.inspect_volume(&volume).await {
        Ok(value) => {
            own(Some(&value.labels), installation);
            docker
                .remove_volume(&volume, None::<RemoveVolumeOptions>)
                .await
                .unwrap();
        }
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {}
        Err(error) => panic!("owned volume inspection failed: {error}"),
    }
    match docker
        .inspect_network(&base, None::<InspectNetworkOptions>)
        .await
    {
        Ok(value) => {
            own(value.labels.as_ref(), installation);
            docker.remove_network(&base).await.unwrap();
        }
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {}
        Err(error) => panic!("owned network inspection failed: {error}"),
    }
}
#[tokio::test]
#[ignore = "requires explicit owned Docker CE image and local restricted database fixture"]
async fn mail_only_native_core_daemon_handles_lost_acks_and_retains_instance() {
    fixture(Scenario::Lifecycle).await;
}
#[tokio::test]
#[ignore = "requires built Console, owned Docker CE image and local restricted database fixture"]
async fn native_console_drives_incoming_mail_activation_and_retained_suspension() {
    assert_eq!(
        std::env::var("WEBDOCK_MAIL_BROWSER_TEST").as_deref(),
        Ok("1")
    );
    fixture(Scenario::Browser).await;
}
#[tokio::test]
#[ignore = "requires built Console, owned Docker CE image and local restricted database fixture"]
async fn native_console_applies_mail_domains_mailboxes_and_access() {
    assert_eq!(
        std::env::var("WEBDOCK_MAIL_BROWSER_TEST").as_deref(),
        Ok("1")
    );
    fixture(Scenario::BrowserConfiguration).await;
}
async fn fixture(scenario: Scenario) {
    assert_eq!(
        std::env::var("WEBDOCK_MAIL_NATIVE_DOCKER_TEST").as_deref(),
        Ok("1")
    );
    let image = std::env::var("WEBDOCK_MAIL_RUNTIME_IMAGE").unwrap();
    assert!(webdock_supervisor::image_digest(&image));
    let mut url =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    let admin = Database::connect(url.as_str()).await.unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("webdock_native_mail_{suffix}_test");
    let installation = format!("mail-{}", &suffix[..12]);
    admin
        .execute_unprepared(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    url.set_path(&format!("/{name}"));
    let docker =
        Docker::connect_with_unix("/var/run/docker.sock", 10, bollard::API_DEFAULT_VERSION)
            .unwrap()
            .negotiate_version()
            .await
            .unwrap();
    let result =
        std::panic::AssertUnwindSafe(run(&url, &name, &installation, &image, &docker, scenario))
            .catch_unwind()
            .await;
    cleanup(&docker, &installation).await;
    admin
        .execute_unprepared(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    admin.close().await.unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
async fn run(
    url: &url::Url,
    name: &str,
    installation: &str,
    image: &str,
    docker: &Docker,
    scenario: Scenario,
) {
    let configure = matches!(scenario, Scenario::BrowserConfiguration);
    let owner = Database::connect(url.as_str()).await.unwrap();
    webdock_persistence::migrations::bootstrap(&owner, name, KEY)
        .await
        .unwrap();
    sql(&owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES('1001','Operator','operator@example.invalid',true,'operator',true,false)",vec![]).await;
    sql(&owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES('operator-session','operator-token','1001',now()+interval '1 hour',now())",vec![]).await;
    let tx = owner.begin().await.unwrap();
    sql(&tx,"SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"123\",\"organization_id\":\"124\"}',true)",vec![]).await;
    sql(
        &tx,
        "INSERT INTO webdock_admin.customers(id,name) VALUES('123','Native Mail fixture')",
        vec![],
    )
    .await;
    tx.commit().await.unwrap();
    sql(&owner,"INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":true}')",vec![]).await;
    if configure {
        sql(&owner,"INSERT INTO webdock_auth.mail_sender_domain(id,customer_id,domain,token,status) VALUES('301','123','example.test','owned-fixture','ownership_verified')",vec![]).await;
    }
    sql(&owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('mail','builtin','enabled','enabled','owned-fixture')",vec![]).await;
    let capacity = json!({"apps":2,"cpuMillicores":1000,"memoryBytes":2147483648u64,"volumeBytes":21474836480u64,"ephemeralBytes":536870912,"replicasPerApp":1,"concurrentDeployments":2});
    sql(&owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,verified,capacity,runtime_kind,runtime_installation) VALUES('200','Owned local Mail','fixture','eu','local isolated integration fixture',true,$1,'docker',$2)",vec![capacity.into(),installation.into()]).await;
    let credential_hash = hex::encode(Sha256::digest("owned-mail-enrollment-only"));
    sql(&owner,"INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_sequence) VALUES('200',$1,1,0)",vec![credential_hash.clone().into()]).await;
    sql(&owner,"INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid) VALUES($1,'mail-agent',1,'200',$2,'unix',$3)",vec![installation.into(),credential_hash.into(),i64::from(rustix::process::geteuid().as_raw()).into()]).await;
    let auth = role("AUTH_DATABASE_URL", name, url).await;
    let business = role("DATABASE_URL", name, url).await;
    let worker = role("WEBDOCK_PLUGIN_WORKER_DATABASE_URL", name, url).await;
    let cookies = CookiePolicy::new(false, "native-mail-cookie-fixture-key-atleast32").unwrap();
    let cookie = format!(
        "{}={}",
        cookies.name(),
        cookies.sign("operator-token").unwrap()
    );
    let identity = Arc::new(IdentityService::new(auth.clone(), cookies));
    let principal = identity
        .authenticate_cookie(&cookie)
        .await
        .unwrap()
        .unwrap();
    let ids = Arc::new(webdock_persistence::IdAllocator::new(
        role("DATABASE_URL", name, url).await,
        webdock_ids::Identity::platform(0, 2).unwrap(),
    ));
    let hosting = Arc::new(
        webdock_hosting::HostingService::new(
            business.clone(),
            auth.clone(),
            worker.clone(),
            KEY.into(),
            "native-mail-platform-key-atleast32".into(),
            ids.clone(),
        )
        .await
        .unwrap(),
    );
    let incoming = Arc::new(
        NativeIncoming::new(
            MailDependencies {
                auth: auth.clone(),
                business: business.clone(),
                worker: worker.clone(),
                identity: identity.clone(),
                browser: None,
                ids,
            },
            IncomingConfig {
                enabled: true,
                mailboxes_enabled: true,
                sending_enabled: true,
                managed_cluster: "200".into(),
                rls_secret: KEY.into(),
            },
        )
        .await
        .unwrap()
        .with_local_docker_test(),
    );
    let native = Arc::new(
        IncomingAgent::new(
            incoming.clone(),
            MailAgentConfig {
                image: image.into(),
                hostname_suffix: "example.test".into(),
                encryption_key: STANDARD.encode([43u8; 32]),
            },
        )
        .unwrap()
        .with_configuration_enabled(configure),
    );
    let control = ControlPlane::new(
        auth.clone(),
        hosting,
        Some(LocalBinding {
            installation: installation.into(),
            agent_id: "mail-agent".into(),
            uid: rustix::process::geteuid().as_raw(),
        }),
    )
    .await
    .unwrap()
    .with_mail(native)
    .with_hosting_enabled(false);
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = dir.path().join("agent.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let lost = Arc::new(LostAck {
        core: control.clone(),
        checkpoint: AtomicBool::new(true),
        complete: AtomicBool::new(true),
        checkpoints: Mutex::new(vec![]),
        receipts: Mutex::new(vec![]),
    });
    let service = lost.clone();
    let configuration = configuration::Delivery::new(control.clone());
    let configuration_service = configuration.service();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(control.service())
            .add_service(configuration_service)
            .add_service(MailControlServer::new(MailService(service)))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::UnixListenerStream::new(listener),
                async {
                    let _ = shutdown.await;
                },
            )
            .await
            .unwrap();
    });
    let runtime = dir.path().join("mail.json");
    private(
        &runtime,
        &json!({"image":image,"installation":installation}),
    );
    let config = AgentConfig {
        version: 1,
        installation: installation.into(),
        agent_id: "mail-agent".into(),
        generation: 1,
        state_directory: dir.path().into(),
        control: ControlEndpoint::Unix {
            path: socket.clone(),
        },
        runtime: RuntimeConfiguration::Docker {
            socket: "/var/run/docker.sock".into(),
            initializer_image: None,
            storage_socket: None,
        },
        database_runtime: None,
        recovery_runtime: None,
        mail_configuration_enabled: configure,
        mail_runtime: Some(runtime),
        policy: Default::default(),
        workloads: false,
        plugin_runtime_enabled: false,
    };
    let base = format!("wd-mail-{installation}-123");
    let flow=std::panic::AssertUnwindSafe(async{
        if matches!(scenario, Scenario::Browser | Scenario::BrowserConfiguration) {
            browser::run(browser::Fixture {
                incoming: incoming.clone(), identity: identity.clone(),
                business: business.clone(), auth: auth.clone(), config: config.clone(),
                cookie: &cookie, directory: dir.path(), configure,
            }).await;
            assert_eq!(incoming.service(&principal,&StoredAuthority::LocalSession {},"123").await.unwrap().service.state,ServiceState::Ready);
            assert_eq!(docker.inspect_container(&base,None::<InspectContainerOptions>).await.unwrap().state.unwrap().running,Some(true));
            assert!(docker.inspect_volume(&format!("{base}-data")).await.is_ok());
            if configure {
                configuration.verify_exact_replays();
                assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.domain WHERE customer_id='123' AND state='ready' AND provider_id IS NOT NULL AND jsonb_array_length(dkim_keys)>0").await,1);
                assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.mailbox WHERE customer_id='123' AND state='ready' AND provider_id IS NOT NULL").await,1);
                assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.identity WHERE customer_id='123' AND provider_id IS NOT NULL AND encrypted_credentials IS NOT NULL").await,1);
                assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.mailbox_grant WHERE customer_id='123' AND NOT enabled AND revision=applied_revision").await,1);
                assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.configuration WHERE customer_id='123' AND revision=5 AND applied_revision=5").await,1);
            }
            return;
        }
        let mut daemon=Daemon::open(config.clone()).await.unwrap();daemon.connect().await.unwrap();
        assert!(!daemon.cycle().await.unwrap(),"Mail-only idle agent must heartbeat without a Hosting module");
        disabled_hosting_policy(&owner,&socket,installation,image).await;
        assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.service").await,0);
        assert!(matches!(docker.inspect_container(&base,None::<InspectContainerOptions>).await,Err(bollard::errors::Error::DockerResponseServerError{status_code:404,..})));
        incoming.manage_service(&principal,&StoredAuthority::LocalSession {},"123",ServiceCommand::Activate{revision:"0".into()}).await.unwrap();
        assert!(daemon.cycle().await.unwrap());
        assert_eq!(lost.checkpoints.lock().unwrap().len(),1);
        assert!(webdock_agent::outbox::MailOutbox::open(dir.path().join("mail-outbox.json")).unwrap().pending().unwrap().is_some());
        drop(daemon);
        let mut daemon=Daemon::open(config.clone()).await.unwrap();daemon.connect().await.unwrap();
        assert_eq!(lost.checkpoints.lock().unwrap().len(),2);
        {let values=lost.checkpoints.lock().unwrap();assert!(values[0]==values[1],"checkpoint replay changed");}
        assert!(daemon.cycle().await.unwrap());
        assert_eq!(incoming.service(&principal,&StoredAuthority::LocalSession {},"123").await.unwrap().service.state,ServiceState::Ready);
        assert_eq!(lost.receipts.lock().unwrap().len(),1);
        let observed=docker.inspect_container(&base,None::<InspectContainerOptions>).await.unwrap();
        let id=observed.id.clone();let started=observed.state.as_ref().and_then(|state|state.started_at.clone());
        assert!(!observed.config.as_ref().unwrap().env.as_ref().unwrap().iter().any(|value|value.starts_with("STALWART_RECOVERY_")));
        drop(daemon);
        let mut daemon=Daemon::open(config.clone()).await.unwrap();daemon.connect().await.unwrap();
        assert!(!daemon.cycle().await.unwrap());
        assert_eq!(lost.receipts.lock().unwrap().len(),2);
        {let values=lost.receipts.lock().unwrap();assert!(values[0]==values[1],"receipt replay changed");}
        let replayed=docker.inspect_container(&base,None::<InspectContainerOptions>).await.unwrap();
        assert_eq!(replayed.id,id);assert_eq!(replayed.state.as_ref().and_then(|state|state.started_at.clone()),started);
        let before=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT encrypted_credentials,smtp_certificate_sha256 FROM webdock_mail.instance WHERE customer_id='123'")).await.unwrap().unwrap();
        let credentials:String=before.try_get("","encrypted_credentials").unwrap();let certificate:String=before.try_get("","smtp_certificate_sha256").unwrap();
        incoming.manage_service(&principal,&StoredAuthority::LocalSession {},"123",ServiceCommand::Suspend{revision:"1".into()}).await.unwrap();
        assert!(daemon.cycle().await.unwrap());
        assert_eq!(incoming.service(&principal,&StoredAuthority::LocalSession {},"123").await.unwrap().service.state,ServiceState::Suspended);
        assert_eq!(docker.inspect_container(&base,None::<InspectContainerOptions>).await.unwrap().state.unwrap().running,Some(false));
        assert!(docker.inspect_volume(&format!("{base}-data")).await.is_ok());
        incoming.manage_service(&principal,&StoredAuthority::LocalSession {},"123",ServiceCommand::Activate{revision:"2".into()}).await.unwrap();
        assert!(daemon.cycle().await.unwrap());
        assert_eq!(incoming.service(&principal,&StoredAuthority::LocalSession {},"123").await.unwrap().service.state,ServiceState::Ready);
        let after=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT encrypted_credentials,smtp_certificate_sha256 FROM webdock_mail.instance WHERE customer_id='123'")).await.unwrap().unwrap();
        let cipher=webdock_mail::secrets::MailCipher::new(&STANDARD.encode([43u8;32])).unwrap();
        assert!(cipher.open::<serde_json::Value>(&credentials,"instance:123").unwrap()==cipher.open::<serde_json::Value>(&after.try_get::<String>("","encrypted_credentials").unwrap(),"instance:123").unwrap(),"retained credentials changed");
        assert_eq!(after.try_get::<String>("","smtp_certificate_sha256").unwrap(),certificate);
        assert!(webdock_agent::outbox::MailOutbox::open(dir.path().join("mail-outbox.json")).unwrap().pending().unwrap().is_none());
        sql(&owner,"UPDATE webdock_auth.agent_transport_enrollment SET revoked=true WHERE installation=$1",vec![installation.into()]).await;
        assert!(matches!(daemon.cycle().await,Err(webdock_agent::management::AgentError::Revoked)));
        assert_eq!(docker.inspect_container(&base,None::<InspectContainerOptions>).await.unwrap().state.unwrap().running,Some(true));
        drop(daemon);
        sql(&owner,"UPDATE webdock_auth.hosting_agent SET generation=2,last_sequence=0 WHERE cluster_id='200'",vec![]).await;
        sql(&owner,"UPDATE webdock_auth.agent_transport_enrollment SET generation=2,revoked=false,last_sequence=0 WHERE installation=$1",vec![installation.into()]).await;
        let mut reconfigured=config.clone();reconfigured.generation=2;
        let mut daemon=Daemon::open(reconfigured).await.unwrap();daemon.connect().await.unwrap();
        incoming.manage_service(&principal,&StoredAuthority::LocalSession {},"123",ServiceCommand::Suspend{revision:"3".into()}).await.unwrap();
        assert!(daemon.cycle().await.unwrap());
        assert_eq!(incoming.service(&principal,&StoredAuthority::LocalSession {},"123").await.unwrap().service.state,ServiceState::Suspended);
        incoming.manage_service(&principal,&StoredAuthority::LocalSession {},"123",ServiceCommand::Activate{revision:"4".into()}).await.unwrap();
        assert!(daemon.cycle().await.unwrap());
        assert_eq!(incoming.service(&principal,&StoredAuthority::LocalSession {},"123").await.unwrap().service.state,ServiceState::Ready);
        assert_eq!(docker.inspect_container(&base,None::<InspectContainerOptions>).await.unwrap().state.unwrap().running,Some(true));
        drop(daemon);
    }).catch_unwind().await;
    stop.send(()).unwrap();
    server.await.unwrap();
    drop(lost);
    drop(incoming);
    drop(identity);
    owner.close().await.unwrap();
    auth.close().await.unwrap();
    business.close().await.unwrap();
    worker.close().await.unwrap();
    if let Err(panic) = flow {
        std::panic::resume_unwind(panic);
    }
}
