//! Real isolated-domain -> authenticated daemon -> owned CNPG acceptance.
#[path = "support/native_database.rs"]
mod support;
use sea_orm::{ConnectionTrait, Database};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use support::*;
use tonic::{Request, Response, Status};
use webdock_agent::management::Daemon;
use webdock_agent_control::{ControlPlane, LocalBinding};
use webdock_agent_protocol::{
    self as wire,
    database_control_server::{DatabaseControl, DatabaseControlServer},
};
use webdock_databases::{DatabaseService, StoredAuthority};
struct LossyDatabaseControl {
    core: ControlPlane,
    checkpoint: AtomicBool,
    receipt: AtomicBool,
    checkpoints: Mutex<Vec<Vec<u8>>>,
    receipts: Mutex<Vec<Vec<u8>>>,
}
#[derive(Clone)]
struct Service(Arc<LossyDatabaseControl>);
#[tonic::async_trait]
impl DatabaseControl for Service {
    async fn poll_database(
        &self,
        r: Request<wire::PollRequest>,
    ) -> Result<Response<wire::DatabaseDispatch>, Status> {
        self.0.core.poll_database(r).await
    }
    async fn guard_database(
        &self,
        r: Request<wire::DatabaseGuardRequest>,
    ) -> Result<Response<wire::GuardReply>, Status> {
        self.0.core.guard_database(r).await
    }
    async fn database_observation_config(
        &self,
        r: Request<wire::PollRequest>,
    ) -> Result<Response<wire::DatabaseObservationPlan>, Status> {
        self.0.core.database_observation_config(r).await
    }
    async fn report_database_observations(
        &self,
        r: Request<wire::DatabaseObservationRequest>,
    ) -> Result<Response<wire::Ack>, Status> {
        self.0.core.report_database_observations(r).await
    }
    async fn checkpoint_database(
        &self,
        r: Request<wire::DatabaseCheckpointRequest>,
    ) -> Result<Response<wire::Ack>, Status> {
        let bytes = prost::Message::encode_to_vec(r.get_ref());
        let result = self.0.core.checkpoint_database(r).await?;
        self.0.checkpoints.lock().unwrap().push(bytes);
        if self.0.checkpoint.swap(false, Ordering::SeqCst) {
            return Err(Status::unavailable(
                "owned fixture lost checkpoint acknowledgement",
            ));
        }
        Ok(result)
    }
    async fn complete_database(
        &self,
        r: Request<wire::DatabaseReceipt>,
    ) -> Result<Response<wire::Ack>, Status> {
        let bytes = prost::Message::encode_to_vec(r.get_ref());
        let result = self.0.core.complete_database(r).await?;
        self.0.receipts.lock().unwrap().push(bytes);
        if self.0.receipt.swap(false, Ordering::SeqCst) {
            return Err(Status::unavailable(
                "owned fixture lost receipt acknowledgement",
            ));
        }
        Ok(result)
    }
}
async fn drive(
    daemon: &mut Option<Daemon>,
    config: &webdock_agent::config::AgentConfig,
    owner: &sea_orm::DatabaseConnection,
    id: &str,
    expected: &str,
) {
    for _ in 0..180 {
        let result = daemon.as_mut().unwrap().cycle().await;
        let pending = webdock_agent::outbox::DatabaseOutbox::open(
            config.state_directory.join("database-outbox.json"),
        )
        .unwrap()
        .pending()
        .unwrap()
        .is_some();
        if result.is_err() || pending {
            drop(daemon.take());
            let mut restarted = Daemon::open(config.clone()).await.unwrap();
            restarted.connect().await.unwrap();
            *daemon = Some(restarted);
        }
        let current = row(
            owner,
            "SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database d WHERE id=$1",
            id,
        )
        .await;
        assert_ne!(
            current["state"], "needs-reconciliation",
            "native daemon mutation must complete without uncertain fallback"
        );
        if current["state"] == expected {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("native domain operation never reached {expected}");
}
#[tokio::test]
#[ignore = "requires explicit owned Kind root fixture and a verified disposable PostgreSQL target"]
async fn owned_database_domain_daemon_and_cnpg_complete_with_exact_restart_replay() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("webdock_agent=warn")
        .with_ansi(false)
        .with_test_writer()
        .try_init();
    assert_eq!(
        std::env::var("WEBDOCK_DISPOSABLE_DATABASE_TEST").as_deref(),
        Ok("yes")
    );
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let target: std::net::SocketAddr = std::env::var("WEBDOCK_DATABASE_TEST_PG_TARGET")
        .unwrap()
        .parse()
        .unwrap();
    assert!(matches!(target.ip(),std::net::IpAddr::V4(ip) if ip.is_private()));
    assert_eq!(target.port(), 5432);
    let source =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert_eq!(source.host_str(), Some("127.0.0.1"));
    assert_eq!(source.path(), "/webdock_rls_test");
    let listener =
        tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, source.port().unwrap()))
            .await
            .unwrap();
    let forward = tokio::spawn(async move {
        loop {
            let (mut incoming, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let Ok(mut outgoing) = tokio::net::TcpStream::connect(target).await else {
                    return;
                };
                let _ = tokio::io::copy_bidirectional(&mut incoming, &mut outgoing).await;
            });
        }
    });
    let control = Database::connect(source.as_str()).await.unwrap();
    let name = format!("webdock_native_db_{}_test", uuid::Uuid::new_v4().simple());
    control
        .execute_unprepared(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let init_name = name.clone();
    let initialized = tokio::spawn(async move {
        let owner = connection(&init_name, "WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
        webdock_persistence::migrations::bootstrap(
            &owner,
            &init_name,
            &std::env::var("DATABASE_RLS_SECRET").unwrap(),
        )
        .await
        .unwrap();
        let ids = Arc::new(
            webdock_persistence::IdAllocator::connect(
                &url("AUTH_DATABASE_URL", &init_name),
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )
            .await
            .unwrap(),
        );
        let cluster = ids.allocate().await.unwrap().to_string();
        let fixture = RuntimeFixture::new(cluster.clone()).await;
        (owner, ids, cluster, fixture)
    })
    .await;
    let (owner, ids, cluster, fixture) = match initialized {
        Ok(value) => value,
        Err(error) => {
            control
                .execute_unprepared(&format!("DROP DATABASE {name} WITH (FORCE)"))
                .await
                .unwrap();
            forward.abort();
            panic!("owned fixture initialization failed: {error}");
        }
    };
    let backend = Arc::new(Mutex::new(None::<String>));
    let task_backend = backend.clone();
    let base = fixture.base.clone();
    let pool = fixture.pool.clone();
    let node = fixture.node.clone();
    let runtime_path = fixture.runtime.clone();
    let task_owner = owner.clone();
    let task_name = name.clone();
    let test=tokio::spawn(async move{
        let auth=connection(&task_name,"AUTH_DATABASE_URL").await;let business=connection(&task_name,"DATABASE_URL").await;let worker=connection(&task_name,"WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await;
        let (identity,actor)=identity(auth.clone(),ids.clone()).await;
        let secret=std::env::var("BETTER_AUTH_SECRET").unwrap();let context_secret=std::env::var("DATABASE_RLS_SECRET").unwrap();
        let hosting=Arc::new(webdock_hosting::HostingService::new(business.clone(),auth.clone(),worker.clone(),context_secret.clone(),secret.clone(),ids.clone()).await.unwrap());
        let service=Arc::new(DatabaseService::new(business,auth.clone(),worker,identity,None,ids.clone(),context_secret,&secret,"http://127.0.0.1:3150").await.unwrap().with_management(webdock_databases::ManagementConfig{cluster_id:Some(cluster.clone()),endpoint_domain:Some("fixture.test".into()),public_ordering:false,protected_database_id:None,lease_secret:Some(secret)}).unwrap());
        exec(&task_owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('hosting','builtin','enabled','enabled','owned-native-test'),('databases','builtin','enabled','enabled','owned-native-test')",vec![]).await;
        let customer=ids.allocate().await.unwrap().to_string();let project=ids.allocate().await.unwrap().to_string();
        exec(&task_owner,"WITH allocation AS (SELECT set_config('webdock.organization_allocation',$2,true)) INSERT INTO webdock_admin.customers(id,name) SELECT $1,'Owned native database fixture' FROM allocation",vec![customer.clone().into(),json!({"customer_id":customer,"organization_id":ids.allocate().await.unwrap().to_string()}).to_string().into()]).await;
        exec(&task_owner,"INSERT INTO webdock_admin.projects(id,customer_id,name) VALUES($1,$2,'Owned native database fixture')",vec![project.clone().into(),customer.into()]).await;
        let installation=format!("db-{}",&cluster[cluster.len()-12..]);let credential_hash=hex::encode(Sha256::digest(format!("owned-native-{cluster}")));
        exec(&task_owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,verified,capacity,runtime_kind,runtime_installation) VALUES($1,'Owned native database cluster','fixture','local','owned local Kind acceptance',true,$2,'kubernetes',$3)",vec![cluster.clone().into(),json!({"apps":100,"cpuMillicores":64000,"memoryBytes":1099511627776_u64,"volumeBytes":2147483648_u64,"ephemeralBytes":17179869184_u64,"replicasPerApp":10,"concurrentDeployments":10}).into(),installation.clone().into()]).await;
        exec(&task_owner,"INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_sequence,observation) VALUES($1,$2,1,0,'{}')",vec![cluster.clone().into(),credential_hash.clone().into()]).await;
        exec(&task_owner,"INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid) VALUES($1,'database-fixture',1,$2,$3,'unix',0)",vec![installation.clone().into(),cluster.clone().into(),credential_hash.into()]).await;
        let core=ControlPlane::new(auth,hosting,Some(LocalBinding{installation:installation.clone(),agent_id:"database-fixture".into(),uid:0})).await.unwrap().with_databases(service.clone());
        let lossy=Arc::new(LossyDatabaseControl{core:core.clone(),checkpoint:AtomicBool::new(true),receipt:AtomicBool::new(true),checkpoints:Mutex::new(vec![]),receipts:Mutex::new(vec![])});
        let socket=base.join("control.sock");let listener=tokio::net::UnixListener::bind(&socket).unwrap();std::fs::set_permissions(&socket,std::fs::Permissions::from_mode(0o600)).unwrap();let (stop,shutdown)=tokio::sync::oneshot::channel();let rpc=lossy.clone();
        let server=tokio::spawn(async move{tonic::transport::Server::builder().add_service(core.service()).add_service(DatabaseControlServer::new(Service(rpc))).serve_with_incoming_shutdown(tokio_stream::wrappers::UnixListenerStream::new(listener),async{let _=shutdown.await;}).await.unwrap();});
        let state=base.join("agent");std::fs::create_dir(&state).unwrap();std::fs::set_permissions(&state,std::fs::Permissions::from_mode(0o700)).unwrap();
        let chart=std::path::PathBuf::from(std::env::var("WEBDOCK_DATABASE_TEST_CHART").unwrap());
        let mut config=webdock_agent::config::AgentConfig{version:1,installation,agent_id:"database-fixture".into(),generation:1,state_directory:state,control:webdock_agent::config::ControlEndpoint::Unix{path:socket},runtime:webdock_agent::config::RuntimeConfiguration::Kubernetes{kubeconfig:std::env::var("WEBDOCK_KUBE_TEST_CONFIG").unwrap().into(),context:std::env::var("WEBDOCK_KUBE_TEST_CONTEXT").unwrap(),helm:std::env::var("WEBDOCK_DATABASE_TEST_HELM").unwrap().into(),chart_sha256:webdock_agent::kubernetes::ApprovedChart::digest(&chart).unwrap(),chart,storage_socket:None,storage_node:None,public_routing:None},database_runtime:Some(runtime_path),mail_runtime:None,recovery_runtime:None,mail_configuration_enabled:false,policy:Default::default(),workloads:false,plugin_runtime_enabled:false};
        let mut daemon=Daemon::open(config.clone()).await.unwrap();daemon.connect().await.unwrap();assert!(!daemon.cycle().await.unwrap());let mut daemon=Some(daemon);
        let authority=StoredAuthority::LocalSession{};
        service.execute_managed(&actor,&authority,json!({"action":"databases.pools.register","clusterID":cluster,"node":node,"poolID":pool,"revision":1,"idempotencyKey":"owned-native-pool-register"})).await.unwrap();
        let plan=service.execute_managed(&actor,&authority,json!({"action":"databases.plans.create","plan":{"kind":"dedicated","postgresMajor":18,"cpuMillicores":500,"memoryBytes":536870912,"volumeBytes":536870912,"logicalStorageAllowanceBytes":null,"name":"Owned native plan","monthlyPriceCents":1200,"currency":"EUR","connectionLimit":10,"terms":"Owned disposable integration fixture database service terms."},"idempotencyKey":"owned-native-plan-create"})).await.unwrap();
        service.execute_managed(&actor,&authority,json!({"action":"databases.plans.setAvailability","planRevisionID":plan["id"],"availability":"pilot","revision":1,"idempotencyKey":"owned-native-plan-availability"})).await.unwrap();
        let offer=service.execute_managed(&actor,&authority,json!({"action":"databases.offers.create","projectID":project,"planRevisionID":plan["id"],"name":"Owned native database","idempotencyKey":"owned-native-offer-create"})).await.unwrap();
        let accepted=service.execute_managed(&actor,&authority,json!({"action":"databases.acceptOrder","offerID":offer["id"],"projectID":project,"idempotencyKey":"owned-native-order-accept"})).await.unwrap();
        let database=accepted["databaseID"].as_str().unwrap();let record=row(&task_owner,"SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database d WHERE id=$1",database).await;let backend_id=record["backend_id"].as_str().unwrap().to_owned();*task_backend.lock().unwrap()=Some(backend_id.clone());
        use std::io::Write;let host_marker=format!("webdock-native-domain-{cluster}");let host_line=format!("\n{} db-{backend_id}.fixture.test # {host_marker}\n",std::env::var("WEBDOCK_DATABASE_TEST_NODE_IP").unwrap());std::fs::OpenOptions::new().append(true).open("/etc/hosts").unwrap().write_all(host_line.as_bytes()).unwrap();
        eprintln!("native domain action: create");
        drive(&mut daemon,&config,&task_owner,database,"ready").await;
        {let checkpoints=lossy.checkpoints.lock().unwrap();assert!(checkpoints.len()>=2);assert_eq!(checkpoints[0],checkpoints[1]);}
        {let receipts=lossy.receipts.lock().unwrap();assert_eq!(receipts.len(),2);assert_eq!(receipts[0],receipts[1]);}
        let physical=row(&task_owner,"SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1",&backend_id).await;
        exec(&task_owner,"UPDATE webdock_auth.agent_transport_enrollment SET revoked=true WHERE cluster_id=$1",vec![cluster.clone().into()]).await;
        assert!(matches!(daemon.as_mut().unwrap().cycle().await,Err(webdock_agent::management::AgentError::Revoked)));
        let resource=kube::api::ApiResource::from_gvk(&kube::api::GroupVersionKind::gvk("postgresql.cnpg.io","v1","Cluster"));
        let clusters:kube::Api<kube::api::DynamicObject>=kube::Api::namespaced_with(client().await,&format!("wd-db-{backend_id}"),&resource);
        let retained=clusters.get("postgres").await.unwrap();assert_eq!(retained.metadata.uid.as_deref(),physical["cnpg_uid"].as_str());assert_eq!(retained.data["status"]["readyInstances"],1);
        drop(daemon.take());
        exec(&task_owner,"UPDATE webdock_auth.hosting_agent SET generation=2,last_sequence=0 WHERE cluster_id=$1",vec![cluster.clone().into()]).await;
        exec(&task_owner,"UPDATE webdock_auth.agent_transport_enrollment SET generation=2,revoked=false,last_sequence=0,last_seen=NULL WHERE cluster_id=$1",vec![cluster.clone().into()]).await;
        config.generation=2;let mut reenrolled=Daemon::open(config.clone()).await.unwrap();reenrolled.connect().await.unwrap();assert!(!reenrolled.cycle().await.unwrap());daemon=Some(reenrolled);
        for (action,expected) in [("restart","ready"),("rotateCredentials","ready"),("pause","paused"),("resume","ready"),("delete","retained"),("purge","purged")]{
            eprintln!("native domain action: {action}");
            let record=row(&task_owner,"SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database d WHERE id=$1",database).await;
            let mut command=json!({"action":format!("databases.{action}"),"databaseID":database,"revision":record["revision"],"idempotencyKey":format!("owned-native-maintenance-{action}")});
            if action=="purge"{exec(&task_owner,"UPDATE webdock_auth.hosting_database SET retained_until=now()-interval '1 second' WHERE id=$1",vec![database.into()]).await;}
            if ["delete","purge"].contains(&action){let preview=service.execute_managed(&actor,&authority,json!({"action":if action=="delete"{"databases.deletionPreview"}else{"databases.purgePreview"},"databaseID":database})).await.unwrap();command["planHash"]=preview["planHash"].clone();command["confirmName"]=record["name"].clone();}
            service.execute_managed(&actor,&authority,command).await.unwrap();drive(&mut daemon,&config,&task_owner,database,expected).await;
        }
        drop(daemon);let _=stop.send(());server.await.unwrap();
        let hosts=std::fs::read_to_string("/etc/hosts").unwrap().lines().filter(|l|!l.ends_with(&format!("# {host_marker}"))).collect::<Vec<_>>().join("\n")+"\n";std::fs::write("/etc/hosts",hosts).unwrap();
        eprintln!("fresh restricted domain -> enrolled Core -> daemon -> CNPG: exact lost-ack restart replay, revocation retention and operator reenrollment, ready/restart/rotation/pause/resume/retained delete/purge");
    }).await;
    let backend = backend.lock().unwrap().clone();
    let marker = format!("# webdock-native-domain-{}", fixture.cluster);
    let hosts = std::fs::read_to_string("/etc/hosts")
        .unwrap()
        .lines()
        .filter(|line| !line.ends_with(&marker))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write("/etc/hosts", hosts).unwrap();
    let cleanup = tokio::spawn(fixture.cleanup(backend)).await;
    owner.close().await.unwrap();
    control
        .execute_unprepared(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    control.close().await.unwrap();
    forward.abort();
    let _ = forward.await;
    cleanup.unwrap();
    test.unwrap();
}
