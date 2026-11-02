//! Explicitly owned local qualification; every readiness receipt comes from NativeDatabaseExecutor.
use super::support::{exec, identity, row};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use webdock_agent::database::{DatabaseAuthority, DatabaseStep, NativeDatabaseExecutor};
use webdock_agent_control::{ControlPlane, LocalBinding};
use webdock_agent_protocol::{self as wire, database::*};
use webdock_databases::{DatabaseService, StoredAuthority};
use webdock_supervisor::ExecutionError;

type Client = wire::database_control_client::DatabaseControlClient<tonic::transport::Channel>;
fn private(path: &str, value: &Value) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn read(path: &str) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap()
}
fn kubectl(args: &[&str], input: Option<&str>) -> String {
    use std::io::Write;
    let mut c = std::process::Command::new("kubectl");
    c.args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut p = c.spawn().unwrap();
    if let Some(s) = input {
        p.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
    }
    let o = p.wait_with_output().unwrap();
    assert!(
        o.status.success(),
        "owned kubectl failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap().trim().to_owned()
}
fn object(kind: &str, name: &str, ns: &str) -> Value {
    serde_json::from_str(&kubectl(&["get", kind, name, "-n", ns, "-o", "json"], None)).unwrap()
}
fn sql(ns: &str, database: &str, statement: &str) -> String {
    kubectl(
        &[
            "exec",
            "-i",
            "-n",
            ns,
            "postgres-1",
            "-c",
            "postgres",
            "--",
            "psql",
            "-X",
            "-U",
            "postgres",
            "-Atq",
            "-v",
            "ON_ERROR_STOP=1",
            "-d",
            database,
        ],
        Some(statement),
    )
}
fn owned_failed_pod<'a>(pods: &'a Value, job: &Value) -> Option<&'a Value> {
    pods["items"].as_array()?.iter().find(|pod| {
        pod["status"]["phase"] == "Failed"
            && pod["metadata"]["ownerReferences"]
                .as_array()
                .is_some_and(|owners| {
                    owners
                        .iter()
                        .any(|owner| owner["uid"] == job["metadata"]["uid"])
                })
    })
}
async fn wait_absent(ns: &str) {
    let end = Instant::now() + Duration::from_secs(180);
    loop {
        let pods: Value =
            serde_json::from_str(&kubectl(&["get", "pods", "-n", ns, "-o", "json"], None)).unwrap();
        if pods["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| {
                p["spec"]["volumes"].as_array().is_some_and(|v| {
                    v.iter()
                        .any(|v| v["persistentVolumeClaim"]["claimName"] == "postgres-1")
                })
            })
            .all(|p| matches!(p["status"]["phase"].as_str(), Some("Succeeded" | "Failed")))
        {
            return;
        }
        assert!(Instant::now() < end, "source did not become unavailable");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
#[derive(Clone)]
struct RpcAuthority {
    client: Client,
    identity: wire::AgentIdentity,
}
#[async_trait::async_trait]
impl DatabaseAuthority for RpcAuthority {
    async fn guard(&self, c: &DatabaseClaim) -> webdock_supervisor::Result<()> {
        let response = self
            .client
            .clone()
            .guard_database(wire::DatabaseGuardRequest {
                identity: Some(self.identity.clone()),
                spec_json: serde_json::to_vec(&c.spec).unwrap(),
                request_sha256: c.spec.fingerprint().unwrap(),
                lease_json: serde_json::to_vec(&c.lease).unwrap(),
            })
            .await
            .map_err(|_| ExecutionError::Denied)?;
        if response.into_inner().allowed {
            Ok(())
        } else {
            Err(ExecutionError::Denied)
        }
    }
    async fn checkpoint(
        &self,
        c: &DatabaseClaim,
        p: DatabaseCheckpoint,
    ) -> webdock_supervisor::Result<()> {
        let response = self
            .client
            .clone()
            .checkpoint_database(wire::DatabaseCheckpointRequest {
                identity: Some(self.identity.clone()),
                spec_json: serde_json::to_vec(&c.spec).unwrap(),
                request_sha256: c.spec.fingerprint().unwrap(),
                checkpoint_json: serde_json::to_vec(&p).unwrap(),
            })
            .await
            .map_err(|_| ExecutionError::Denied)?;
        if response.into_inner().accepted {
            Ok(())
        } else {
            Err(ExecutionError::Denied)
        }
    }
}
struct Harness {
    owner: DatabaseConnection,
    service: Arc<DatabaseService>,
    actor: webdock_identity::Principal,
    authority: RpcAuthority,
    native: NativeDatabaseExecutor,
    cluster: String,
    pool: String,
    config: Value,
    storage_stop: tokio::sync::oneshot::Sender<()>,
    server_stop: tokio::sync::oneshot::Sender<()>,
    storage: tokio::task::JoinHandle<webdock_supervisor::Result<()>>,
    server: tokio::task::JoinHandle<()>,
}
impl Harness {
    async fn open(phase: &str) -> Self {
        let config = read("/connected/config.json");
        let connect = |key: &str| std::env::var(key).unwrap();
        let owner = Database::connect(connect("WEBDOCK_TEST_MIGRATION_DATABASE_URL"))
            .await
            .unwrap();
        let auth = Database::connect(connect("AUTH_DATABASE_URL"))
            .await
            .unwrap();
        let business = Database::connect(connect("DATABASE_URL")).await.unwrap();
        let worker = Database::connect(connect("WEBDOCK_PLUGIN_WORKER_DATABASE_URL"))
            .await
            .unwrap();
        let ids = Arc::new(
            webdock_persistence::IdAllocator::connect(
                &connect("AUTH_DATABASE_URL"),
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )
            .await
            .unwrap(),
        );
        let (identity, actor) = identity(auth.clone(), ids.clone()).await;
        let existing = phase == "restore" || Path::new("/connected/setup.json").exists();
        let selection = if phase == "restore" {
            read("/connected/restore-selection.json")
        } else if existing {
            read("/connected/setup.json")
        } else {
            json!({})
        };
        let cluster = if existing {
            text(&selection, "cluster").into()
        } else {
            ids.allocate().await.unwrap().to_string()
        };
        let pool = if existing {
            text(&selection, "pool").into()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        let secret = connect("BETTER_AUTH_SECRET");
        let rls = connect("DATABASE_RLS_SECRET");
        let hosting = Arc::new(
            webdock_hosting::HostingService::new(
                business.clone(),
                auth.clone(),
                worker.clone(),
                rls.clone(),
                secret.clone(),
                ids.clone(),
            )
            .await
            .unwrap(),
        );
        let service = Arc::new(
            DatabaseService::new(
                business,
                auth.clone(),
                worker,
                identity,
                None,
                ids,
                rls,
                &secret,
                "http://127.0.0.1:3150",
            )
            .await
            .unwrap()
            .with_management(webdock_databases::ManagementConfig {
                cluster_id: Some(cluster.clone()),
                endpoint_domain: Some("fixture.test".into()),
                public_ordering: false,
                protected_database_id: None,
                lease_secret: Some(secret),
            })
            .unwrap(),
        );
        if !existing {
            exec(&owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('hosting','builtin','enabled','enabled','fixture'),('databases','builtin','enabled','enabled','fixture')",vec![]).await;
            let cap = json!({"apps":10,"cpuMillicores":6000,"memoryBytes":8589934592_u64,"volumeBytes":9663676416_u64,"ephemeralBytes":4294967296_u64,"replicasPerApp":1,"concurrentDeployments":2});
            exec(&owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,verified,capacity,runtime_kind,runtime_installation) VALUES($1,'Connected backup','fixture','local','fresh isolated backup qualification node',true,$2,'kubernetes','backup-fixture')",vec![cluster.clone().into(),cap.into()]).await;
            let hash = hex::encode(Sha256::digest("owned-connected-fixture-enrollment"));
            exec(&owner,"INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_sequence,last_seen,observation) VALUES($1,$2,1,0,now(),'{}')",vec![cluster.clone().into(),hash.clone().into()]).await;
            exec(&owner,"INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid) VALUES('backup-fixture','backup-agent',1,$1,$2,'unix',0)",vec![cluster.clone().into(),hash.into()]).await;
        }
        let storage_config = json!({"root":"/connected/volumes","node":"backup-qualification","clusterID":cluster,"poolID":pool,"capacityBytes":9663676416_u64,"reserveBytes":268435456});
        private("/connected/storage.json", &storage_config);
        if !existing {
            std::fs::create_dir("/connected/volumes").unwrap();
            std::fs::set_permissions("/connected/volumes", std::fs::Permissions::from_mode(0o700))
                .unwrap();
            webdock_agent::storage::StoragePool::initialize(Path::new("/connected/storage.json"))
                .unwrap();
        }
        std::fs::create_dir_all("/run/webdock-postgres-storage").unwrap();
        std::fs::set_permissions(
            "/run/webdock-postgres-storage",
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let (storage_stop, shutdown) = tokio::sync::oneshot::channel();
        let storage = tokio::spawn(async move {
            webdock_agent::storage::serve_kind(
                Path::new("/connected/storage.json"),
                Path::new("/run/webdock-postgres-storage/connected.sock"),
                webdock_agent::storage::StorageKind::Database,
                async {
                    let _ = shutdown.await;
                },
            )
            .await
        });
        for _ in 0..200 {
            if Path::new("/run/webdock-postgres-storage/connected.sock").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let runtime = json!({"image":config["versions"]["POSTGRES_IMAGE"],"poolerImage":config["versions"]["PGBOUNCER_IMAGE"],"issuer":"connected-ca","apiCIDRs":["10.43.0.1/32",format!("{}/32",text(&config,"nodeIP"))],"podCIDRs":["10.42.0.0/16"],"storageSocket":"/run/webdock-postgres-storage/connected.sock","caFile":"/connected/ca.crt","backup":{"pluginImage":config["images"]["plugin"],"sidecarImage":config["images"]["sidecar"],"endpoints":[config["endpoint"]],"egressCIDRs":[format!("{}/32",text(&config,"nodeIP"))],"endpointCACertificate":std::fs::read_to_string("/connected/ca.crt").unwrap()}});
        private("/connected/runtime.json", &runtime);
        let client = kube::Client::try_from(
            kube::Config::from_custom_kubeconfig(
                kube::config::Kubeconfig::read_from("/etc/rancher/k3s/k3s.yaml").unwrap(),
                &Default::default(),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        let native =
            NativeDatabaseExecutor::open(client, "/connected/runtime.json".into()).unwrap();
        let socket = Path::new("/connected/control.sock");
        if socket.exists() {
            std::fs::remove_file(socket).unwrap();
        }
        let listener = tokio::net::UnixListener::bind(socket).unwrap();
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let core = ControlPlane::new(
            auth,
            hosting.clone(),
            Some(LocalBinding {
                installation: "backup-fixture".into(),
                agent_id: "backup-agent".into(),
                uid: 0,
            }),
        )
        .await
        .unwrap()
        .with_databases(service.clone());
        let (server_stop, shutdown) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(core.database_service())
                .serve_with_incoming_shutdown(
                    tokio_stream::wrappers::UnixListenerStream::new(listener),
                    async {
                        let _ = shutdown.await;
                    },
                )
                .await
                .unwrap();
        });
        let channel = tonic::transport::Endpoint::from_shared("unix:///connected/control.sock")
            .unwrap()
            .connect()
            .await
            .unwrap();
        let authority = RpcAuthority {
            client: Client::new(channel),
            identity: wire::AgentIdentity {
                installation: "backup-fixture".into(),
                agent_id: "backup-agent".into(),
                generation: 1,
            },
        };
        private(
            "/connected/setup.json",
            &json!({"cluster":cluster,"pool":pool}),
        );
        let h = Self {
            owner,
            service,
            actor,
            authority,
            native,
            cluster,
            pool,
            config,
            storage_stop,
            server_stop,
            storage,
            server,
        };
        h.observe().await;
        h
    }
    async fn command(&self, v: Value) -> Value {
        let action = v["action"].clone();
        self.service
            .execute_managed(&self.actor, &StoredAuthority::LocalSession {}, v)
            .await
            .unwrap_or_else(|e| panic!("managed {action}: {e}"))
    }
    async fn observe(&self) {
        let c = self
            .native
            .capabilities(&["backup-qualification".into()])
            .await
            .unwrap()
            .expect("real operator, admission and storage must be qualified");
        assert_eq!(c.database_backup_version, Some(1));
        let c = serde_json::to_value(c).unwrap();
        let cap = json!({"apps":10,"cpuMillicores":6000,"memoryBytes":8589934592_u64,"volumeBytes":9663676416_u64,"ephemeralBytes":4294967296_u64,"replicasPerApp":1,"concurrentDeployments":2});
        let mut app_capacity = cap;
        app_capacity["volumeBytes"] = json!(0);
        let value = json!({"capacity":app_capacity,"nodes":[{"id":"backup-qualification","cpuMillicores":6000,"memoryBytes":8589934592_u64}],"runtimeKind":"kubernetes","capabilities":{"networkIsolation":true,"restrictedPods":true,"storage":false,"databaseVersion":c["databaseVersion"],"databaseBackupVersion":c["databaseBackupVersion"],"databasePools":c["databasePools"],"databaseNodePorts":c["databaseNodePorts"]}});
        exec(&self.owner,"UPDATE webdock_auth.hosting_agent SET observation=$2,last_seen=now() WHERE cluster_id=$1",vec![self.cluster.clone().into(),value.into()]).await;
    }
    async fn poll(&self) -> Option<DatabaseClaim> {
        let raw = self
            .authority
            .client
            .clone()
            .poll_database(wire::PollRequest {
                identity: Some(self.authority.identity.clone()),
            })
            .await
            .unwrap()
            .into_inner()
            .command?;
        Some(webdock_agent::management::database_claim(&raw, 1).unwrap())
    }
    async fn finish(&self, operation: &str) -> Value {
        self.finish_inner(operation, false).await
    }
    async fn finish_inner(&self, operation: &str, provider_fault: bool) -> Value {
        let stored=row(&self.owner,"SELECT jsonb_build_object('status',status,'proof',result,'spec',desired->'nativeIssuedSpecs'->-1->'spec') AS value FROM webdock_auth.hosting_operation WHERE id=$1",operation).await;
        if stored["status"] == "succeeded" {
            return json!({"resumedStoredNativeReceipt":true,"proof":stored["proof"],"spec":stored["spec"]});
        }
        let start = Instant::now();
        let mut stages = vec![];
        let mut key_resumed = !provider_fault;
        loop {
            self.observe().await;
            let claim = self
                .poll()
                .await
                .expect("accepted operation must be claimable");
            assert_eq!(claim.spec.operation_id, operation);
            let ns = &claim.spec.backend.namespace;
            // DNS is private to this disposable node and generated product names only.
            let hosts = std::fs::read_to_string("/etc/hosts").unwrap();
            if !hosts.contains(&claim.spec.backend.host) {
                use std::io::Write;
                std::fs::OpenOptions::new()
                    .append(true)
                    .open("/etc/hosts")
                    .unwrap()
                    .write_all(
                        format!(
                            "\n{} {}\n",
                            text(&self.config, "nodeIP"),
                            claim.spec.backend.host
                        )
                        .as_bytes(),
                    )
                    .unwrap();
            }
            let mut custody = if Path::new("/connected/custody.json").exists() {
                read("/connected/custody.json")
            } else {
                json!({"namespaces":[]})
            };
            if !custody["namespaces"]
                .as_array()
                .unwrap()
                .contains(&json!(ns))
            {
                custody["namespaces"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(ns));
                private("/connected/custody.json", &custody);
            }
            let stage = format!("{:?}", claim.spec.phase);
            if stages.last().is_none_or(|v: &Value| v["phase"] != stage) {
                stages.push(json!({"phase":stage,"seconds":start.elapsed().as_secs_f64()}));
                eprintln!("connected {:?} {:?}", claim.spec.action, claim.spec.phase);
            }
            match self
                .native
                .execute(&claim, &self.authority)
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "native {:?} {:?}: {e:?}",
                        claim.spec.action, claim.spec.phase
                    )
                }) {
                DatabaseStep::Pending => {
                    if !key_resumed && claim.spec.phase == DatabaseExecutionPhase::Configure {
                        let jobs: Value = serde_json::from_str(&kubectl(
                            &["get", "jobs", "-n", ns, "-o", "json"],
                            None,
                        ))
                        .unwrap();
                        if let Some(job) = jobs["items"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|j| j["status"]["failed"].as_u64().unwrap_or(0) > 0)
                        {
                            let pods: Value = serde_json::from_str(&kubectl(
                                &["get", "pods", "-n", ns, "-o", "json"],
                                None,
                            ))
                            .unwrap();
                            let failed = owned_failed_pod(&pods, job)
                                .expect("failed recovery Job must have an owned failed pod");
                            let log = kubectl(
                                &[
                                    "logs",
                                    text(&failed["metadata"], "name"),
                                    "-n",
                                    ns,
                                    "-c",
                                    "plugin-barman-cloud",
                                    "--tail=150",
                                ],
                                None,
                            );
                            std::fs::write("/connected/revoked-key-failure.log", &log).unwrap();
                            std::fs::set_permissions(
                                "/connected/revoked-key-failure.log",
                                std::fs::Permissions::from_mode(0o600),
                            )
                            .unwrap();
                            assert!(
                                ["403", "AccessDenied", "InvalidAccessKeyId"]
                                    .iter()
                                    .any(|v| log.contains(v)),
                                "private recovery job log must prove provider credential rejection"
                            );
                            let state=row(&self.owner,"SELECT jsonb_build_object('state',d.state,'billingStartedAt',o.billing_started_at) AS value FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_order o ON o.id=d.order_id WHERE d.id=$1",&claim.spec.database.as_ref().unwrap().id).await;
                            assert_eq!(state["state"], "restoring");
                            assert!(state["billingStartedAt"].is_null());
                            private(
                                "/connected/key-revocation.json",
                                &json!({"failedRecoveryJobUID":job["metadata"]["uid"],"observedCoreState":state,"providerRejected":true,"failureLogSHA256":hex::encode(Sha256::digest(log.as_bytes()))}),
                            );
                            self.provider_key("resume").await;
                            key_resumed = true;
                        }
                    }
                }
                DatabaseStep::Complete(proof) => {
                    assert!(
                        key_resumed,
                        "provider failure must be observed before a readiness receipt"
                    );
                    let receipt = wire::DatabaseReceipt {
                        identity: Some(self.authority.identity.clone()),
                        spec_json: serde_json::to_vec(&claim.spec).unwrap(),
                        request_sha256: claim.spec.fingerprint().unwrap(),
                        completion_json: serde_json::to_vec(&DatabaseCompletion {
                            lease: claim.lease.clone(),
                            outcome: DatabaseCompletionOutcome::Succeeded,
                            proof: Some(proof.clone()),
                        })
                        .unwrap(),
                    };
                    assert!(
                        self.authority
                            .client
                            .clone()
                            .complete_database(receipt)
                            .await
                            .unwrap()
                            .into_inner()
                            .accepted
                    );
                    return json!({"seconds":start.elapsed().as_secs_f64(),"stages":stages,"proof":proof,"spec":claim.spec});
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(650),
                "connected native operation timed out"
            );
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
    async fn order(&self, project: &str, plan: &Value, name: &str, key: &str) -> Value {
        let existing=self.owner.query_one_raw(sea_orm::Statement::from_sql_and_values(sea_orm::DbBackend::Postgres,"SELECT jsonb_build_object('orderID',d.order_id,'databaseID',d.id,'operationID',d.operation_id) AS value FROM webdock_auth.hosting_database d WHERE d.project_id=$1 AND d.name=$2 AND d.plan_revision_id=$3 ORDER BY d.id DESC LIMIT 1",[project.into(),name.into(),text(plan,"id").into()])).await.unwrap();
        if let Some(existing) = existing {
            return existing.try_get("", "value").unwrap();
        }
        let offer=self.command(json!({"action":"databases.offers.create","projectID":project,"planRevisionID":plan["id"],"name":name,"idempotencyKey":format!("connected-offer-{key}")})).await;
        self.command(json!({"action":"databases.acceptOrder","offerID":offer["id"],"projectID":project,"idempotencyKey":format!("connected-order-{key}")})).await
    }
    async fn provider_key(&self, action: &str) {
        let request = json!({"nonce":self.config["nonce"],"action":action,"requestID":uuid::Uuid::new_v4().to_string()});
        private("/connected/fault-request.json", &request);
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            if Path::new("/connected/fault-ack.json").exists()
                && read("/connected/fault-ack.json") == request
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "owned MinIO fault-controller did not acknowledge"
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    async fn wrong_cipher(&self) -> DatabaseService {
        let auth = Database::connect(std::env::var("AUTH_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let business = Database::connect(std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let worker =
            Database::connect(std::env::var("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").unwrap())
                .await
                .unwrap();
        let ids = Arc::new(
            webdock_persistence::IdAllocator::connect(
                &std::env::var("AUTH_DATABASE_URL").unwrap(),
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )
            .await
            .unwrap(),
        );
        let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
        let identity = Arc::new(
            webdock_identity::IdentityService::new(
                auth.clone(),
                webdock_identity::CookiePolicy::new(false, secret.clone()).unwrap(),
            )
            .with_secret_keys(
                webdock_identity::SecretKeys::new(
                    std::collections::BTreeMap::from([(0, secret.clone())]),
                    0,
                    Some(secret.clone()),
                )
                .unwrap(),
            ),
        );
        DatabaseService::new(
            business,
            auth,
            worker,
            identity,
            None,
            ids,
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            "wrong-fixture-recovery-master-key-32-bytes",
            "http://127.0.0.1:3150",
        )
        .await
        .unwrap()
        .with_management(webdock_databases::ManagementConfig {
            cluster_id: Some(self.cluster.clone()),
            endpoint_domain: Some("fixture.test".into()),
            public_ordering: false,
            protected_database_id: None,
            lease_secret: Some(secret),
        })
        .unwrap()
    }
    async fn close(self) {
        let _ = self.storage_stop.send(());
        let _ = self.server_stop.send(());
        self.storage.await.unwrap().unwrap();
        self.server.await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "fresh connected fixture only; run qualification_connected.py"]
async fn core_authenticated_grpc_native_backup_and_recovery() {
    assert_eq!(
        std::env::var("WEBDOCK_CONNECTED_BACKUP_TEST").as_deref(),
        Ok("yes")
    );
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let phase = std::env::var("WEBDOCK_CONNECTED_BACKUP_PHASE").unwrap();
    let h = Harness::open(&phase).await;
    if phase == "backup" {
        backup(&h).await;
    } else if phase.starts_with("review-") {
        review::run(&h, &phase).await;
    } else {
        assert_eq!(phase, "restore");
        restore(&h).await;
    }
    h.close().await;
}
async fn backup(h: &Harness) {
    let source = if Path::new("/connected/source-order.json").exists() {
        read("/connected/source-order.json")
    } else {
        let customer = h.service_test_id().await;
        let project = h.service_test_id().await;
        let organization = h.service_test_id().await;
        exec(&h.owner,"WITH allocation AS (SELECT set_config('webdock.organization_allocation',$2,true)) INSERT INTO webdock_admin.customers(id,name) SELECT $1,'Connected backup fixture' FROM allocation",vec![customer.clone().into(),json!({"customer_id":customer,"organization_id":organization}).to_string().into()]).await;
        exec(&h.owner,"INSERT INTO webdock_admin.projects(id,customer_id,name) VALUES($1,$2,'Connected backup fixture')",vec![project.clone().into(),customer.clone().into()]).await;
        h.command(json!({"action":"databases.pools.register","clusterID":h.cluster,"node":"backup-qualification","poolID":h.pool,"revision":1,"idempotencyKey":"connected-pool-register"})).await;
        let plan=h.command(json!({"action":"databases.plans.create","plan":{"kind":"dedicated","postgresMajor":18,"cpuMillicores":250,"memoryBytes":536870912,"volumeBytes":1610612736_u64,"logicalStorageAllowanceBytes":null,"name":"Connected local plan","monthlyPriceCents":1200,"currency":"EUR","connectionLimit":10,"terms":"Disposable connected backup fixture. No production service or recovery commitment."},"idempotencyKey":"connected-plan-create"})).await;
        h.command(json!({"action":"databases.plans.setAvailability","planRevisionID":plan["id"],"availability":"pilot","revision":1,"idempotencyKey":"connected-plan-available"})).await;
        let accepted = h.order(&project, &plan, "Source", "source").await;
        json!({"customer":customer,"project":project,"plan":plan,"accepted":accepted})
    };
    let mut source = source;
    let snapshot = row(
        &h.owner,
        "SELECT o.snapshot AS value FROM webdock_auth.hosting_database_order o WHERE o.id=$1",
        text(&source["accepted"], "orderID"),
    )
    .await;
    let offer_hash = hex::encode(Sha256::digest(serde_json::to_vec(&snapshot).unwrap()));
    if let Some(prior) = source.get("offerSnapshotSHA256") {
        assert_eq!(prior, &json!(offer_hash));
    }
    source["offerSnapshotSHA256"] = json!(offer_hash);
    private("/connected/source-order.json", &source);
    let customer = text(&source, "customer");
    let project = text(&source, "project");
    let plan = &source["plan"];
    let accepted = &source["accepted"];
    let create = h.finish(text(accepted, "operationID")).await;
    let database = text(accepted, "databaseID");
    let ns = text(&create["spec"]["backend"], "namespace");
    let dbname = text(&create["spec"]["database"], "name");
    sql(
        ns,
        dbname,
        "CREATE TABLE IF NOT EXISTS app.sentinels(id text PRIMARY KEY,value text NOT NULL); ALTER TABLE app.sentinels OWNER TO CURRENT_USER;",
    );
    sql(
        ns,
        dbname,
        &format!(
            "ALTER TABLE app.sentinels OWNER TO wd_migrate_{database}; GRANT SELECT,INSERT,UPDATE,DELETE ON app.sentinels TO wd_app_{database}; INSERT INTO app.sentinels VALUES('A','before-target') ON CONFLICT DO NOTHING;"
        ),
    );
    let existing=row(&h.owner,"SELECT jsonb_build_object('grant',(SELECT jsonb_build_object('policyID',p.id,'operationID',o.id) FROM webdock_auth.hosting_database_backup_policy p JOIN webdock_auth.hosting_database_job j ON j.backup_policy_id=p.id JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id WHERE p.database_id=$1 AND j.backup_policy_revision=p.revision ORDER BY o.created_at DESC LIMIT 1)) AS value",database).await;
    let (grant, schedule) = if !existing["grant"].is_null() {
        let grant = existing["grant"].clone();
        let schedule = object(
            "scheduledbackup",
            &format!("backup-{}", text(&grant, "policyID")),
            ns,
        )["spec"]["schedule"]
            .as_str()
            .unwrap()
            .to_owned();
        (grant, schedule)
    } else {
        let destination=h.command(json!({"action":"databases.backupDestinations.register","endpointURL":h.config["endpoint"],"region":"us-east-1","bucket":h.config["bucket"],"keyRecoveryID":"110","writeCredentials":{"accessKeyId":h.config["write"]["access"],"secretAccessKey":h.config["write"]["secret"],"readOnly":false},"recoveryCredentials":{"accessKeyId":h.config["read"]["access"],"secretAccessKey":h.config["read"]["secret"],"readOnly":true},"idempotencyKey":"connected-destination-register"})).await;
        let now = chrono::Utc::now() + chrono::Duration::minutes(2);
        let schedule = format!("0 {} {} * * *", now.format("%M"), now.format("%H"));
        let grant=h.command(json!({"action":"databases.backupPolicies.grant","databaseID":database,"revision":1,"destinationID":destination["destinationID"],"schedule":schedule,"retentionDays":30,"retentionDelete":false,"idempotencyKey":"connected-backup-grant"})).await;

        (grant, schedule)
    };
    let revoked=row(&h.owner,"SELECT jsonb_build_object('revoked',p.revoked_at IS NOT NULL) AS value FROM webdock_auth.hosting_database_backup_policy p WHERE p.id=$1",text(&grant,"policyID")).await;
    if revoked["revoked"] == true {
        resume_revoked_backup(h, &source, &create, &grant, &schedule).await;
        return;
    }
    // Expire the real authenticated initiating browser before any native policy apply.
    exec(
        &h.owner,
        "UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 minute' WHERE id=$1",
        vec![h.actor.session_id.clone().into()],
    )
    .await;
    assert!(
        h.service
            .execute_managed(
                &h.actor,
                &StoredAuthority::LocalSession {},
                json!({"action":"databases.backupPolicies.get","databaseID":database})
            )
            .await
            .is_err()
    );
    let configure = h.finish(text(&grant, "operationID")).await;
    let policy = object(
        "scheduledbackup",
        &format!("backup-{}", text(&grant, "policyID")),
        ns,
    );
    assert_eq!(policy["spec"]["schedule"], schedule);
    assert_eq!(policy["spec"]["suspend"], false);
    // Recurrence comes from installed CNPG RBAC and the real daily product schedule.
    let end = Instant::now() + Duration::from_secs(240);
    let base;
    loop {
        let backups: Value =
            serde_json::from_str(&kubectl(&["get", "backups", "-n", ns, "-o", "json"], None))
                .unwrap();
        if let Some(b) = backups["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["status"]["phase"] == "completed")
        {
            base = b.clone();
            break;
        }
        assert!(
            Instant::now() < end,
            "daily recurrence did not complete after browser expiry"
        );
        h.observe().await;
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    // The durable observer sees the real completed scheduled Backup, never a fabricated Ready.
    exec(&h.owner,"UPDATE webdock_auth.hosting_database_backup_policy SET observed_at=now()-interval '61 seconds' WHERE id=$1",vec![text(&grant,"policyID").into()]).await;
    let observation = h.poll().await.unwrap();
    assert_eq!(observation.spec.phase, DatabaseExecutionPhase::Observe);
    let recorded = h.finish(&observation.spec.operation_id).await;
    let ids = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &std::env::var("AUTH_DATABASE_URL").unwrap(),
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let (_, operator) = identity(
        Database::connect(std::env::var("AUTH_DATABASE_URL").unwrap())
            .await
            .unwrap(),
        ids,
    )
    .await;
    let local = StoredAuthority::LocalSession {};
    let command = |v| h.service.execute_managed(&operator, &local, v);
    let manual=command(json!({"action":"databases.backup","databaseID":database,"revision":1,"idempotencyKey":"connected-manual-backup"})).await.unwrap();
    let manual = h.finish(text(&manual, "operationID")).await;
    let target = sql(
        ns,
        "postgres",
        r#"SELECT to_char(clock_timestamp() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"');"#,
    );
    let digest = sql(
        ns,
        dbname,
        include_str!("../../../agent/src/database/backup-digest.sql"),
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    sql(
        ns,
        dbname,
        "INSERT INTO app.sentinels VALUES('B','after-target');",
    );
    let rotate=command(json!({"action":"databases.rotateCredentials","databaseID":database,"revision":1,"idempotencyKey":"connected-source-password-rotate"})).await.unwrap();
    let rotated = h.finish(text(&rotate, "operationID")).await;
    // Fixture workload shaping only; no forced final WAL switch or lowered TLS verification.
    kubectl(
        &[
            "patch",
            "cluster",
            "postgres",
            "-n",
            ns,
            "--type=merge",
            "-p",
            r#"{"spec":{"postgresql":{"parameters":{"archive_timeout":"10s"}}}}"#,
        ],
        None,
    );
    tokio::time::sleep(Duration::from_secs(25)).await;
    let mut controls = vec![];
    for (revision, wal, retention) in [(1, true, true), (2, false, false)] {
        let update=command(json!({"action":"databases.backupPolicies.update","databaseID":database,"revision":2,"policyRevision":revision,"schedule":schedule,"suspended":true,"walEnabled":wal,"retentionDelete":retention,"retentionDays":30,"idempotencyKey":format!("connected-controls-{revision}")})).await.unwrap();
        controls.push(h.finish(text(&update, "operationID")).await);
    }
    let pending=command(json!({"action":"databases.backupPolicies.update","databaseID":database,"revision":2,"policyRevision":3,"schedule":schedule,"suspended":true,"walEnabled":false,"retentionDelete":false,"retentionDays":30,"idempotencyKey":"connected-before-revocation"})).await.unwrap();
    let stale = h.poll().await.unwrap();
    assert_eq!(stale.spec.operation_id, pending["operationID"]);
    let revoke=command(json!({"action":"databases.backupPolicies.revoke","databaseID":database,"revision":2,"policyRevision":4,"idempotencyKey":"connected-explicit-revocation"})).await.unwrap();
    assert_eq!(revoke["revocationEffective"], false);
    assert!(h.authority.guard(&stale).await.is_err());
    let before=row(&h.owner,"SELECT jsonb_build_object('checkpoint',j.checkpoint,'status',o.status,'leaseUntil',o.lease_until,'generation',o.generation) AS value FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_database_job j ON j.operation_id=o.id WHERE o.id=$1",&stale.spec.operation_id).await;
    // Transport custody acknowledges obsolete delivery so the outbox can drain;
    // it must not install a checkpoint, renew a lease or authorize execution.
    h.authority
        .checkpoint(
            &stale,
            DatabaseCheckpoint {
                lease: stale.lease.clone(),
                phase: DatabaseCheckpointPhase::Applied,
                cnpg_uid: text(&create["proof"], "cnpgUID").into(),
                ca_certificate: None,
                previous_pod_uid: None,
                migration_job_uid: None,
                copy_sha256: None,
            },
        )
        .await
        .unwrap();
    let after=row(&h.owner,"SELECT jsonb_build_object('checkpoint',j.checkpoint,'status',o.status,'leaseUntil',o.lease_until,'generation',o.generation) AS value FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_database_job j ON j.operation_id=o.id WHERE o.id=$1",&stale.spec.operation_id).await;
    assert_eq!(before, after);
    let revoked = h.finish(text(&revoke, "operationID")).await;
    let status = command(json!({"action":"databases.backupPolicies.get","databaseID":database}))
        .await
        .unwrap();
    assert_eq!(status["policy"]["revocationEffective"], true);
    let versions=row(&h.owner,"SELECT jsonb_agg(revision ORDER BY revision) AS value FROM webdock_auth.hosting_database_backup_credentials WHERE policy_id=$1",text(&grant,"policyID")).await;
    assert_eq!(versions, json!([1, 2]));
    // Source becomes unavailable before any Core metadata is exported for independent recovery.
    kubectl(
        &[
            "annotate",
            "cluster",
            "postgres",
            "-n",
            ns,
            "cnpg.io/hibernation=on",
            "--overwrite",
        ],
        None,
    );
    wait_absent(ns).await;
    let backups=row(&h.owner,"SELECT jsonb_agg(to_jsonb(b)) AS value FROM webdock_auth.hosting_database_backup b WHERE policy_id=$1",text(&grant,"policyID")).await;
    let restore_point = backups
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["base_backup_id"] == manual["proof"]["backup"]["baseBackupId"])
        .unwrap();
    assert_eq!(
        recorded["proof"]["backup"]["baseBackupId"],
        base["status"]["backupId"]
    );
    private(
        "/connected/restore-selection.json",
        &json!({"cluster":h.cluster,"pool":h.pool,"customer":customer,"project":project,"plan":plan,"database":database,"namespace":ns,"sourceUID":create["proof"]["cnpgUID"],"targetTime":target,"digest":digest,"restorePointID":restore_point["id"],"baseBackupID":restore_point["base_backup_id"],"sourceRevision":2,"offerSnapshotSHA256":source["offerSnapshotSHA256"],"sourceOrderID":accepted["orderID"],"policyID":grant["policyID"]}),
    );
    let final_snapshot = row(
        &h.owner,
        "SELECT o.snapshot AS value FROM webdock_auth.hosting_database_order o WHERE o.id=$1",
        text(accepted, "orderID"),
    )
    .await;
    assert_eq!(
        hex::encode(Sha256::digest(serde_json::to_vec(&final_snapshot).unwrap())),
        offer_hash
    );
    private(
        "/connected/evidence.json",
        &json!({"fixtureOnly":true,"originalOfferSnapshotUnchanged":true,"offerSnapshotSHA256":offer_hash,"connectedCreate":create,"connectedPolicy":configure,"connectedScheduledBackup":recorded,"connectedManualBackup":manual,"historicalCredentialRevisions":versions,"rotatedCredentials":rotated,"independentObservedControls":controls,"revocation":revoked,"staleExecutionDeniedAndCallbackRetired":true,"browserExpiredBeforeApply":true,"dailyScheduleObserved":policy["spec"]["schedule"],"sourceUnavailable":true}),
    );
}
async fn resume_revoked_backup(
    h: &Harness,
    source: &Value,
    create: &Value,
    grant: &Value,
    schedule: &str,
) {
    let database = text(&source["accepted"], "databaseID");
    let ns = text(&create["spec"]["backend"], "namespace");
    let dbname = text(&create["spec"]["database"], "name");
    let revoked = h.finish(text(grant, "operationID")).await;
    let history=row(&h.owner,"SELECT jsonb_agg(jsonb_build_object('action',j.action,'policyRevision',j.backup_policy_revision,'operationID',o.id,'proof',o.result,'spec',o.desired->'nativeIssuedSpecs'->-1->'spec') ORDER BY o.created_at) AS value FROM webdock_auth.hosting_database_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id WHERE j.database_id=$1 AND o.status='succeeded'",database).await;
    let history = history.as_array().unwrap();
    let find = |action: &str| {
        history
            .iter()
            .find(|v| v["action"] == action)
            .unwrap()
            .clone()
    };
    let configure = history
        .iter()
        .find(|v| v["action"] == "configureBackup" && v["policyRevision"] == 1)
        .unwrap()
        .clone();
    let scheduled = history
        .iter()
        .find(|v| {
            v["action"] == "configureBackup" && !v["proof"]["backup"]["baseBackupId"].is_null()
        })
        .unwrap()
        .clone();
    let manual = find("backup");
    let rotated = find("rotateCredentials");
    let controls = history
        .iter()
        .filter(|v| v["policyRevision"] == 2 || v["policyRevision"] == 3)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(controls.len(), 2);
    let point=row(&h.owner,"SELECT jsonb_build_object('id',id,'baseBackupID',base_backup_id,'completedAt',completed_at) AS value FROM webdock_auth.hosting_database_backup WHERE policy_id=$1 ORDER BY completed_at DESC LIMIT 1",text(grant,"policyID")).await;
    assert_eq!(
        point["baseBackupID"],
        manual["proof"]["backup"]["baseBackupId"]
    );
    let target = chrono::DateTime::parse_from_rfc3339(text(&point, "completedAt")).unwrap()
        + chrono::Duration::seconds(1);
    let definition = include_str!("../../../agent/src/database/backup-digest.sql")
        .split("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;")
        .next()
        .unwrap();
    let digest = sql(
        ns,
        dbname,
        &format!(
            "{definition} BEGIN; DELETE FROM app.sentinels WHERE id='B'; SELECT pg_temp.webdock_restore_digest(); ROLLBACK;"
        ),
    );
    assert_eq!(
        sql(ns, dbname, "SELECT id FROM app.sentinels ORDER BY id;"),
        "A\nB"
    );
    let disposition=row(&h.owner,"SELECT jsonb_build_object('disposition',o.desired->'nativeDatabaseRetiredDeliveries'->0->>'disposition','checkpoint',j.checkpoint,'status',o.status) AS value FROM webdock_auth.hosting_database_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id WHERE j.backup_policy_id=$1 AND j.backup_policy_revision=4",text(grant,"policyID")).await;
    assert_eq!(disposition["disposition"], "obsolete");
    assert!(disposition["checkpoint"].is_null());
    assert_eq!(disposition["status"], "failed");
    let versions=row(&h.owner,"SELECT jsonb_agg(revision ORDER BY revision) AS value FROM webdock_auth.hosting_database_backup_credentials WHERE policy_id=$1",text(grant,"policyID")).await;
    assert_eq!(versions, json!([1, 2]));
    let status = h
        .command(json!({"action":"databases.backupPolicies.get","databaseID":database}))
        .await;
    assert_eq!(status["policy"]["revocationEffective"], true);
    kubectl(
        &[
            "annotate",
            "cluster",
            "postgres",
            "-n",
            ns,
            "cnpg.io/hibernation=on",
            "--overwrite",
        ],
        None,
    );
    wait_absent(ns).await;
    private(
        "/connected/restore-selection.json",
        &json!({"cluster":h.cluster,"pool":h.pool,"customer":source["customer"],"project":source["project"],"plan":source["plan"],"database":database,"namespace":ns,"sourceUID":create["proof"]["cnpgUID"],"targetTime":target.to_rfc3339(),"targetSelection":"completed base plus one second; before the later B write and rotation","digest":digest,"restorePointID":point["id"],"baseBackupID":point["baseBackupID"],"sourceRevision":2,"offerSnapshotSHA256":source["offerSnapshotSHA256"],"sourceOrderID":source["accepted"]["orderID"],"policyID":grant["policyID"]}),
    );
    let snapshot=row(&h.owner,"SELECT jsonb_build_object('order',o.snapshot,'offer',f.snapshot) AS value FROM webdock_auth.hosting_database_order o JOIN webdock_auth.hosting_database_offer f ON f.id=o.offer_id WHERE o.id=$1",text(&source["accepted"],"orderID")).await;
    assert_eq!(snapshot["order"], snapshot["offer"]);
    private(
        "/connected/evidence.json",
        &json!({"fixtureOnly":true,"resumedAfterTransportAssertionCorrection":true,"originalOfferSnapshotUnchanged":true,"offerSnapshotSHA256":source["offerSnapshotSHA256"],"connectedCreate":create,"connectedPolicy":configure,"connectedScheduledBackup":scheduled,"connectedManualBackup":manual,"historicalCredentialRevisions":versions,"rotatedCredentials":rotated,"independentObservedControls":controls,"revocation":revoked,"staleExecutionDeniedAndCallbackRetired":disposition,"browserExpiredBeforeApply":true,"dailyScheduleObserved":schedule,"sourceUnavailable":true}),
    );
}
impl Harness {
    async fn service_test_id(&self) -> String {
        // Use the real allocator through its own bootstrap role, as in the existing fixtures.
        let ids = webdock_persistence::IdAllocator::connect(
            &std::env::var("AUTH_DATABASE_URL").unwrap(),
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap();
        ids.allocate().await.unwrap().to_string()
    }
}
async fn restore(h: &Harness) {
    assert!(
        h.config.get("write").is_none() && h.config.get("read").is_none(),
        "recovery cannot borrow source credential configuration"
    );
    let s = read("/connected/restore-selection.json");
    let ns = text(&s, "namespace");
    wait_absent(ns).await;
    assert_eq!(
        object("cluster", "postgres", ns)["metadata"]["uid"],
        s["sourceUID"]
    );
    let request = |accepted: &Value, key: &str, name: &str| json!({"action":"databases.restore","sourceDatabaseID":s["database"],"restorePointID":s["restorePointID"],"targetProjectID":s["project"],"targetOrderID":accepted["orderID"],"revision":s["sourceRevision"],"credentialRevision":1,"targetTime":chrono::DateTime::parse_from_rfc3339(text(&s,"targetTime")).unwrap().to_rfc3339(),"timeline":1,"verificationSHA256":s["digest"],"name":name,"idempotencyKey":key});
    h.provider_key("revoke").await;
    let accepted = h
        .order(
            text(&s, "project"),
            &s["plan"],
            "Recovery key fault",
            "key-fault",
        )
        .await;
    let restore_request = request(
        &accepted,
        "connected-restore-key-fault",
        "Recovery key fault",
    );
    let target_state=row(&h.owner,"SELECT jsonb_build_object('state',state) AS value FROM webdock_auth.hosting_database WHERE id=$1",text(&accepted,"databaseID")).await;
    let resumed = ["restoring", "needs-reconciliation", "ready"]
        .iter()
        .any(|state| target_state["state"] == *state);
    if target_state["state"] == "needs-reconciliation" {
        h.command(json!({"action":"databases.reconcile","databaseID":accepted["databaseID"],"revision":1,"idempotencyKey":format!("connected-restore-reconcile-{}",uuid::Uuid::new_v4())})).await;
    }
    let restored = if resumed {
        accepted.clone()
    } else {
        let wrong = h.wrong_cipher().await;
        let wrong_key_error = wrong
            .execute_managed(
                &h.actor,
                &StoredAuthority::LocalSession {},
                restore_request.clone(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            wrong_key_error.status, 503,
            "wrong independently retained key: {}",
            wrong_key_error.message
        );
        private(
            "/connected/wrong-key-proof.json",
            &json!({"rejected":true,"status":503}),
        );
        h.command(restore_request).await
    };
    let fault_started = Instant::now();
    let retained_failure = resumed && Path::new("/connected/key-revocation.json").exists();
    if retained_failure {
        h.provider_key("resume").await;
    }
    let faulted = h
        .finish_inner(text(&restored, "operationID"), !retained_failure)
        .await;
    let fault_seconds = fault_started.elapsed().as_secs_f64();
    // Measure a separate successful path, including fresh target acceptance/reservation.
    let started = Instant::now();
    let accepted = h
        .order(text(&s, "project"), &s["plan"], "Recovered", "restored")
        .await;
    let restore_request = request(&accepted, "connected-restore-accept", "Recovered");
    let capacity = row(
        &h.owner,
        "SELECT capacity AS value FROM webdock_auth.hosting_cluster WHERE id=$1",
        &h.cluster,
    )
    .await;
    exec(&h.owner,"UPDATE webdock_auth.hosting_cluster SET capacity=jsonb_set(capacity,'{ephemeralBytes}','0') WHERE id=$1",vec![h.cluster.clone().into()]).await;
    assert_eq!(
        h.service
            .execute_managed(
                &h.actor,
                &StoredAuthority::LocalSession {},
                restore_request.clone()
            )
            .await
            .unwrap_err()
            .status,
        409,
        "insufficient restore capacity must fail before a target is claimed"
    );
    exec(
        &h.owner,
        "UPDATE webdock_auth.hosting_cluster SET capacity=$2 WHERE id=$1",
        vec![h.cluster.clone().into(), capacity.into()],
    )
    .await;
    let request_started = Instant::now();
    let restored = h.command(restore_request.clone()).await;
    let accepted_at = Instant::now();
    let accepted_seconds = accepted_at.duration_since(started).as_secs_f64();
    let reservation_seconds = accepted_at.duration_since(request_started).as_secs_f64();
    let claim = h.poll().await.unwrap();
    let mut foreign = claim.spec.clone();
    foreign
        .recovery
        .as_mut()
        .unwrap()
        .source
        .destination
        .archive_path = "postgresql/foreign".into();
    assert!(
        h.authority
            .client
            .clone()
            .guard_database(wire::DatabaseGuardRequest {
                identity: Some(h.authority.identity.clone()),
                spec_json: serde_json::to_vec(&foreign).unwrap(),
                request_sha256: claim.spec.fingerprint().unwrap(),
                lease_json: serde_json::to_vec(&claim.lease).unwrap()
            })
            .await
            .is_err(),
        "a foreign archive cannot cross authenticated Core issued-spec authority"
    );
    let result = h.finish(text(&restored, "operationID")).await;
    let native_ready = Instant::now();
    let rto = native_ready.duration_since(started).as_secs_f64();
    let request_rto = native_ready.duration_since(request_started).as_secs_f64();
    let accepted_rto = native_ready.duration_since(accepted_at).as_secs_f64();
    let target_ns = text(&result["spec"]["backend"], "namespace");
    assert_ne!(target_ns, ns);
    assert_ne!(result["proof"]["cnpgUID"], s["sourceUID"]);
    assert_eq!(
        sql(
            target_ns,
            text(&result["spec"]["database"], "name"),
            "SELECT id FROM app.sentinels ORDER BY id;"
        ),
        "A"
    );
    let state=row(&h.owner,"SELECT jsonb_build_object('state',state,'confirmedCredentialRevision',confirmed_credential_revision) AS value FROM webdock_auth.hosting_database WHERE id=$1",text(&restored,"databaseID")).await;
    assert_eq!(state["state"], "ready");
    assert_eq!(state["confirmedCredentialRevision"], 1);
    let mut evidence = read("/connected/evidence.json");
    evidence["connectedRestore"] = json!({"workflowFromTargetOrderSeconds":rto,"restoreRequestThroughNativeProductReadinessSeconds":request_rto,"acceptedRestoreThroughProductVerificationSeconds":accepted_rto,"restoreRequestThroughIndependentAssertionsSeconds":request_started.elapsed().as_secs_f64(),"restoreReservationAndAcceptanceSeconds":reservation_seconds,"targetOrderThroughAcceptanceSeconds":accepted_seconds,"execution":result,"sourceRuntimeUnavailable":true,"sourceCoreDestroyed":true,"privateRecoveryBundleOnly":true,"historicalCredentialRevision":1,"sourceLatestCredentialRevision":2,"wrongMasterKeyRejected":true,"foreignArchiveRPCRejected":true,"capacityRejectedBeforeClaim":true,"A_present":true,"B_absent":true,"targetState":"ready","request":restore_request});
    evidence["revokedKeyConnectedRestore"] = json!({"currentExecutionSeconds":fault_seconds,"resumedEarlierAcceptedRestore":resumed,"observedFailure":read("/connected/key-revocation.json"),"eventuallyVerifiedExecution":faulted});
    private("/connected/evidence.json", &evidence);
}

#[path = "connected_review.rs"]
mod review;

#[path = "connected_export.rs"]
mod export_qualification;
