//! Only unique records in the explicitly owned local RLS fixture; no real cluster calls.
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use spitzli_hosting::{AgentIdentity, HostingService};
use spitzli_identity::{CookiePolicy, IdentityService, Principal};
use std::sync::Arc;
async fn db(key: &str) -> DatabaseConnection {
    let value = std::env::var(key).expect("explicit local fixture URL");
    let parsed = url::Url::parse(&value).unwrap();
    assert!(
        matches!(parsed.host_str(), Some("localhost" | "127.0.0.1"))
            && parsed.path() == "/webdock_rls_test"
            && parsed.query().is_none()
    );
    let mut options = sea_orm::ConnectOptions::new(value);
    if key == "WEBDOCK_TEST_MIGRATION_DATABASE_URL" {
        options.max_connections(1);
    }
    Database::connect(options).await.unwrap()
}
async fn sql(db: &DatabaseConnection, text: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        text,
        values,
    ))
    .await
    .unwrap();
}
async fn id(db: &DatabaseConnection) -> String {
    db.query_one_raw(Statement::from_string(
        DbBackend::Postgres,
        "SELECT webdock_admin.next_snowflake() AS id",
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "id")
    .unwrap()
}
fn spec() -> Value {
    json!({"template":"custom","image":"docker.io/library/busybox@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e","args":[],"port":8080,"healthPath":"/","cpuMillicores":100,"memoryBytes":16777216,"ephemeralBytes":1048576,"replicas":1,"volumeBytes":0})
}
struct Fixture {
    owner: DatabaseConnection,
    service: Arc<HostingService>,
    principal: Principal,
    agent: AgentIdentity,
    uid: String,
    sid: String,
    cid: String,
    pid: String,
    cluster: String,
    mapped: String,
    module_added: bool,
    module_version: String,
    nonce: String,
}
impl Fixture {
    async fn new(runtime: &str) -> Self {
        let owner = db("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
        sql(
            &owner,
            "SELECT pg_advisory_lock(hashtext('webdock-hosting-rust-fixture'))",
            vec![],
        )
        .await;
        let nonce = format!(
            "rust-hosting-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );
        let uid = nonce.clone();
        let sid = format!("{nonce}-session");
        let token = format!("{nonce}-token");
        let cid = id(&owner).await;
        let pid = id(&owner).await;
        let cluster = id(&owner).await;
        let mapped = id(&owner).await;
        sql(&owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES($1,$1,$2,true,'operator',true,false)",vec![uid.clone().into(),format!("{nonce}@example.invalid").into()]).await;
        sql(&owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES($1,$2,$3,now()+interval '1 hour',now())",vec![sid.clone().into(),token.clone().into(),uid.clone().into()]).await;
        sql(&owner,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,$2,$3,'operator',$2)",vec![mapped.clone().into(),uid.clone().into(),format!("{nonce}@example.invalid").into()]).await;
        sql(
            &owner,
            "INSERT INTO webdock_admin.customers(id,name) VALUES($1,$2)",
            vec![cid.clone().into(), nonce.clone().into()],
        )
        .await;
        sql(
            &owner,
            "INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES($1,$2,$3)",
            vec![pid.clone().into(), nonce.clone().into(), cid.clone().into()],
        )
        .await;
        let cap = json!({"apps":1,"cpuMillicores":1000,"memoryBytes":1073741824,"ephemeralBytes":1073741824,"volumeBytes":1073741824,"replicasPerApp":1,"concurrentDeployments":1});
        sql(&owner,"INSERT INTO webdock_auth.tenant_subscription(customer_id,name,description,base,extras,revision,updated_by) VALUES($1,'Hosting test','Local fixture',$2,'{}',1,$3)",vec![cid.clone().into(),json!({"hosting":cap}).into(),uid.clone().into()]).await;
        let installation = format!("test-{}", &cluster[cluster.len().saturating_sub(12)..]);
        sql(&owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,dedicated_customer_id,generation,verified,capacity,runtime_kind,runtime_installation) VALUES($1,$2,'fixture','local','isolated test evidence',$3,1,true,$4,$5,$6)",vec![cluster.clone().into(),nonce.clone().into(),cid.clone().into(),cap.clone().into(),runtime.into(),installation.into()]).await;
        let observation = json!({"sequence":1,"generation":1,"observedAt":chrono::Utc::now().to_rfc3339(),"version":if runtime=="docker"{"docker-test"}else{"v1.30.0+k3s1"},"runtimeKind":runtime,"nodes":[{"id":"fixture-node","role":"server","architecture":"amd64","cpuMillicores":4000,"memoryBytes":4294967296u64}],"capacity":cap,"capabilities":{"networkIsolation":true,"restrictedPods":true,"storage":runtime!="docker","storageVersion":1,"ingress":false,"environment":runtime!="docker","volumeQuotas":runtime!="docker","replicas":runtime!="docker"}});
        let credential = "a".repeat(43);
        sql(&owner,"INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_sequence,last_seen,observation) VALUES($1,$2,1,1,now(),$3)",vec![cluster.clone().into(),hex::encode(Sha256::digest(credential.as_bytes())).into(),observation.into()]).await;
        let module_version = format!(
            "0.1.0-test-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        );
        let module_added=owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('hosting','builtin','enabled','enabled',$1) ON CONFLICT DO NOTHING",[module_version.clone().into()])).await.unwrap().rows_affected()==1;
        let cookies =
            CookiePolicy::new(false, "fixture-hosting-cookie-key-at-least-32-bytes").unwrap();
        let cookie = format!("{}={}", cookies.name(), cookies.sign(&token).unwrap());
        let identity = IdentityService::new(db("AUTH_DATABASE_URL").await, cookies);
        let principal = identity
            .authenticate_cookie(&cookie)
            .await
            .unwrap()
            .unwrap();
        let service = Arc::new(
            HostingService::new(
                db("DATABASE_URL").await,
                db("AUTH_DATABASE_URL").await,
                db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,
                std::env::var("DATABASE_RLS_SECRET").unwrap(),
                std::env::var("BETTER_AUTH_SECRET").unwrap(),
                Arc::new(spitzli_persistence::IdAllocator::new(
                    db("DATABASE_URL").await,
                    spitzli_ids::Identity::platform(0, 2).unwrap(),
                )),
            )
            .await
            .unwrap(),
        );
        let agent = service
            .authenticate_agent(&cluster, &credential)
            .await
            .unwrap();
        Self {
            owner,
            service,
            principal,
            agent,
            uid,
            sid,
            cid,
            pid,
            cluster,
            mapped,
            module_added,
            module_version,
            nonce,
        }
    }
    async fn cleanup(&self) {
        for table in [
            "hosting_reservation",
            "hosting_operation",
            "hosting_app",
            "hosting_project",
        ] {
            sql(
                &self.owner,
                &format!("DELETE FROM webdock_auth.{table} WHERE project_id=$1"),
                vec![self.pid.clone().into()],
            )
            .await;
        }
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.hosting_audit WHERE subject=$1",
            vec![self.uid.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.hosting_rate WHERE key_hash=$1",
            vec![hex::encode(Sha256::digest(format!("heartbeat:{}", self.cluster))).into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.hosting_agent WHERE cluster_id=$1",
            vec![self.cluster.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.hosting_cluster WHERE id=$1",
            vec![self.cluster.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.tenant_subscription WHERE customer_id=$1",
            vec![self.cid.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_admin.projects WHERE id=$1",
            vec![self.pid.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1",
            vec![self.cid.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_admin.customers WHERE id=$1",
            vec![self.cid.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.organization WHERE name=$1",
            vec![self.nonce.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_admin.users WHERE id=$1",
            vec![self.mapped.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.session WHERE id=$1",
            vec![self.sid.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
            vec![self.uid.clone().into()],
        )
        .await;
        if self.module_added {
            sql(
                &self.owner,
                "DELETE FROM webdock_auth.plugin_installation WHERE id='hosting' AND version=$1",
                vec![self.module_version.clone().into()],
            )
            .await;
        }
        sql(
            &self.owner,
            "SELECT pg_advisory_unlock(hashtext('webdock-hosting-rust-fixture'))",
            vec![],
        )
        .await;
    }
    async fn configure(&self, provider: &str) {
        self.service.execute(&self.principal,json!({"action":"projects.create","projectID":self.pid,"provider":provider,"clusterID":self.cluster,"mode":"selfservice","ownImages":true,"idempotencyKey":"configure-project-1"})).await.unwrap();
    }
    async fn proof(&self, claim: &Value, uid: &str) {
        self.service.agent_request(&self.agent,"complete",json!({"id":claim["id"],"generation":claim["generation"],"outcome":"succeeded","proof":{"appID":claim["desired"]["appID"],"revision":claim["target_revision"],"namespace":format!("wd-{}",self.pid),"uid":uid,"ready":true,"deleted":false,"replicas":claim["desired"]["spec"]["replicas"]}})).await.unwrap();
    }
}
#[tokio::test]
#[ignore = "requires explicit local retained-schema RLS fixture plus offline hosting migration"]
async fn durable_app_create_claim_complete_controls_quotas_and_revocation() {
    let fixture = Arc::new(Fixture::new("kubernetes").await);
    let f = fixture.clone();
    let outcome=tokio::spawn(async move{
 f.configure("k3s").await;
 let project=f.service.execute(&f.principal,json!({"action":"projects.get","projectID":f.pid})).await.unwrap();
 assert_eq!(project["configured"],true);
 assert_eq!(project["publicSitePolicy"],Value::Null,"a project without an eligible public domain policy must still open");
 let hash=hex::encode(Sha256::digest("a".repeat(43)));assert!(f.service.authenticate_enrolled_agent(&f.cluster,1,&hash).await.is_ok());assert!(f.service.authenticate_enrolled_agent(&f.cluster,2,&hash).await.is_err());assert!(f.service.authenticate_enrolled_agent(&f.cluster,1,&"0".repeat(64)).await.is_err());let command=json!({"action":"apps.create","projectID":f.pid,"name":"fixture-app","spec":spec(),"environment":[{"name":"TOKEN","value":"very-secret-value"}],"subscriptionRevision":1,"idempotencyKey":"create-app-request-1"});let created=f.service.execute(&f.principal,command.clone()).await.unwrap();assert_eq!(created["status"],"queued");assert_eq!(f.service.execute(&f.principal,command).await.unwrap(),created);let app=created["appID"].as_str().unwrap();
 assert!(f.service.execute(&f.principal,json!({"action":"apps.stop","appID":app,"revision":1,"subscriptionRevision":1,"idempotencyKey":"stop-while-busy-1"})).await.is_err());
 let claim=f.service.agent_request(&f.agent,"claim",json!({})).await.unwrap();assert_eq!(claim["id"],created["id"]);assert_eq!(claim["desired"]["environment"]["TOKEN"],"very-secret-value");assert!(claim["desired"].get("environmentEncrypted").is_none());assert_eq!(claim["desired"]["quota"]["apps"],1);
 assert!(f.service.agent_request(&f.agent,"guard",json!({"id":claim["id"],"generation":99})).await.is_err());f.service.agent_request(&f.agent,"guard",json!({"id":claim["id"],"generation":claim["generation"]})).await.unwrap();
 assert!(f.service.agent_request(&f.agent,"complete",json!({"id":claim["id"],"generation":claim["generation"],"outcome":"succeeded","proof":{"appID":app,"revision":99,"namespace":format!("wd-{}",f.pid),"uid":"workload-uid","ready":true,"deleted":false,"replicas":1}})).await.is_err());f.proof(&claim,"workload-uid").await;
 let view=f.service.execute(&f.principal,json!({"action":"apps.get","appID":app})).await.unwrap();assert_eq!(view["status"],"ready");assert_eq!(view["environmentNames"],json!(["TOKEN"]));assert!(!view.to_string().contains("very-secret-value"));sql(&f.owner,"UPDATE webdock_auth.\"user\" SET role='user' WHERE id=$1",vec![f.uid.clone().into()]).await;assert!(f.service.execute(&f.principal,json!({"action":"apps.get","appID":app})).await.is_err());assert!(f.service.execute(&f.principal,json!({"action":"clusters.list"})).await.is_err());sql(&f.owner,"UPDATE webdock_auth.\"user\" SET role='operator' WHERE id=$1",vec![f.uid.clone().into()]).await;
 assert!(f.service.execute(&f.principal,json!({"action":"apps.create","projectID":f.pid,"name":"over-quota","spec":spec(),"subscriptionRevision":1,"idempotencyKey":"over-quota-request"})).await.is_err());
 for (action,revision,replicas) in [("apps.stop",1,0),("apps.start",2,1),("apps.restart",3,1),("apps.rollback",4,1)]{let result=f.service.execute(&f.principal,json!({"action":action,"appID":app,"revision":revision,"subscriptionRevision":1,"idempotencyKey":format!("control-request-{revision}")})).await.unwrap();let claim=f.service.agent_request(&f.agent,"claim",json!({})).await.unwrap();assert_eq!(claim["id"],result["id"]);assert_eq!(claim["desired"]["spec"]["replicas"],replicas);f.proof(&claim,"workload-uid").await;}
 let logs=f.service.execute(&f.principal,json!({"action":"apps.logs","appID":app,"idempotencyKey":"logs-request-test1"})).await.unwrap();let claim=f.service.agent_request(&f.agent,"claim",json!({})).await.unwrap();assert_eq!(claim["id"],logs["id"]);f.service.agent_request(&f.agent,"complete",json!({"id":claim["id"],"generation":claim["generation"],"outcome":"succeeded","proof":{"appID":app,"revision":5,"namespace":format!("wd-{}",f.pid),"uid":"workload-uid","ready":true,"deleted":false,"replicas":1,"logs":"value=very-secret-value","logsTruncated":false}})).await.unwrap();let view=f.service.execute(&f.principal,json!({"action":"apps.get","appID":app})).await.unwrap();assert_eq!(view["logs"],"value=[redacted]");
 let op=f.service.execute(&f.principal,json!({"action":"apps.restart","appID":app,"revision":5,"subscriptionRevision":1,"idempotencyKey":"expire-operation-1"})).await.unwrap();let claim=f.service.agent_request(&f.agent,"claim",json!({})).await.unwrap();sql(&f.owner,"UPDATE webdock_auth.hosting_operation SET lease_until=now()-interval '1 second' WHERE id=$1",vec![op["id"].as_str().unwrap().into()]).await;assert!(f.service.agent_request(&f.agent,"guard",json!({"id":claim["id"],"generation":claim["generation"]})).await.is_err());f.service.agent_request(&f.agent,"claim",json!({})).await.unwrap();let current=f.service.execute(&f.principal,json!({"action":"operations.get","operationID":op["id"]})).await.unwrap();assert_eq!(current["status"],"needs-reconciliation");f.service.execute(&f.principal,json!({"action":"apps.reconcile","appID":app,"idempotencyKey":"reconcile-request1"})).await.unwrap();let retry=f.service.agent_request(&f.agent,"claim",json!({})).await.unwrap();assert!(retry["generation"].as_i64().unwrap()>claim["generation"].as_i64().unwrap());
 sql(&f.owner,"UPDATE webdock_auth.\"user\" SET banned=true WHERE id=$1",vec![f.uid.clone().into()]).await;assert!(f.service.agent_request(&f.agent,"guard",json!({"id":retry["id"],"generation":retry["generation"]})).await.is_err());assert!(f.service.execute(&f.principal,json!({"action":"apps.get","appID":app})).await.is_err());
 sql(&f.owner,"UPDATE webdock_auth.hosting_agent SET revoked=true WHERE cluster_id=$1",vec![f.cluster.clone().into()]).await;assert!(f.service.agent_request(&f.agent,"claim",json!({})).await.is_err());
 }).await;
    fixture.cleanup().await;
    outcome.unwrap();
}
#[cfg(feature = "docker-test")]
#[tokio::test]
#[ignore = "requires explicit WEBDOCK_HOSTING_DOCKER_TEST=1 and owned local Docker Engine"]
async fn persisted_docker_commands_execute_with_live_authority_and_uid_fences() {
    use spitzli_supervisor::{Authorizer, Command, ExecutionError, LeaseGuard, RuntimeAdapter};
    assert_eq!(
        std::env::var("WEBDOCK_HOSTING_DOCKER_TEST").as_deref(),
        Ok("1")
    );
    struct Live {
        host: Arc<HostingService>,
        agent: AgentIdentity,
    }
    #[async_trait::async_trait]
    impl Authorizer for Live {
        async fn authorize(&self, command: &Command) -> spitzli_supervisor::Result<()> {
            self.host
                .guard_runtime_command(&self.agent, command)
                .await
                .map_err(|_| ExecutionError::Denied)
        }
    }
    let engine = bollard::Docker::connect_with_unix(
        "/var/run/docker.sock",
        10,
        bollard::API_DEFAULT_VERSION,
    )
    .unwrap()
    .negotiate_version()
    .await
    .unwrap();
    engine
        .inspect_image(spec()["image"].as_str().unwrap())
        .await
        .expect("preload the reviewed local test image; tests do not pull images");
    let fixture = Arc::new(Fixture::new("docker").await);
    let f = fixture.clone();
    let names = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let cleanup_names = names.clone();
    let outcome=tokio::spawn(async move {
  f.configure("docker").await;let mut app_spec=spec();app_spec["args"]=json!(["sh","-c","printf ok >/tmp/index.html; exec httpd -f -p 8080 -h /tmp"]);app_spec["ephemeralBytes"]=json!(16777216);
  let created=f.service.execute(&f.principal,json!({"action":"apps.create","projectID":f.pid,"name":"docker-fixture","spec":app_spec,"subscriptionRevision":1,"idempotencyKey":"docker-create-request"})).await.unwrap();let app=created["appID"].as_str().unwrap();
  let authority=Arc::new(Live{host:f.service.clone(),agent:f.agent.clone()});let mut first_uid=None;
  for(action,revision)in[("initial",0),("apps.stop",1),("apps.start",2),("apps.update",3),("apps.rollback",4)]{
   if action!="initial"{let mut request=json!({"action":action,"appID":app,"revision":revision,"subscriptionRevision":1,"idempotencyKey":format!("docker-control-request-{revision}")});if action=="apps.update"{let mut changed=app_spec.clone();changed["cpuMillicores"]=json!(150);request["spec"]=changed;}f.service.execute(&f.principal,request).await.unwrap();}
   let blocked=if action=="initial"{let id=id(&f.owner).await;sql(&f.owner,"INSERT INTO webdock_auth.hosting_operation(id,subject,actor_session,customer_id,project_id,cluster_id,app_id,action,idempotency_key,payload_hash,status,desired,target_revision,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,'apps.logs','unsupported-queued-log','fixture','queued',$8,1,now()-interval '1 minute')",vec![id.clone().into(),f.uid.clone().into(),f.sid.clone().into(),f.cid.clone().into(),f.pid.clone().into(),f.cluster.clone().into(),app.into(),json!({"action":"logs","spec":app_spec}).into()]).await;Some(id)}else{None};
   let command=f.service.claim_runtime(&f.agent).await.unwrap().unwrap();
   if let Some(blocked)=blocked{let status:String=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT status FROM webdock_auth.hosting_operation WHERE id=$1",[blocked.clone().into()])).await.unwrap().unwrap().try_get("","status").unwrap();assert_eq!(status,"queued");sql(&f.owner,"DELETE FROM webdock_auth.hosting_operation WHERE id=$1",vec![blocked.into()]).await;}
   let mut changed=command.clone();changed.spec.cpu_millicores+=1;assert!(f.service.guard_runtime_command(&f.agent,&changed).await.is_err());
   names.lock().unwrap().push(command.spec.name());let adapter=spitzli_agent::docker::DockerAdapter::connect("/var/run/docker.sock",command.spec.installation.clone(),Some(command.spec.image.clone())).await.unwrap();
   if let Some(old)=&first_uid{assert!(command.expected_runtime_id.is_some());assert_eq!(command.expected_runtime_id.as_ref(),Some(old));}
   let guard=LeaseGuard::new(command.clone(),authority.clone());let observed=adapter.reconcile(&command,&guard).await.unwrap();assert!(observed.ready||command.spec.replicas==0);assert_eq!(observed.resource_id,command.spec.resource_id.to_string(),"resource identity for {action}");assert_eq!(observed.revision,command.spec.revision,"observation revision for {action}");f.service.complete_runtime(&f.agent,&command,&observed).await.unwrap();f.service.complete_runtime(&f.agent,&command,&observed).await.unwrap();first_uid=observed.runtime_id;
  }
  let view=f.service.execute(&f.principal,json!({"action":"apps.get","appID":app})).await.unwrap();assert_eq!(view["status"],"ready");assert_eq!(view["observedRevision"],5);
  let mut unsupported=app_spec.clone();unsupported["volumeBytes"]=json!(67108864);let unavailable=f.service.execute(&f.principal,json!({"action":"apps.create","projectID":f.pid,"name":"unsupported-volume","spec":unsupported,"subscriptionRevision":1,"idempotencyKey":"docker-no-quota-claim"})).await.unwrap_err();assert!(unavailable.message.contains("persistent storage")||unavailable.message.contains("quota"));
  let failed=f.service.execute(&f.principal,json!({"action":"apps.restart","appID":app,"revision":5,"subscriptionRevision":1,"idempotencyKey":"docker-failure-receipt"})).await.unwrap();let command=f.service.claim_runtime(&f.agent).await.unwrap().unwrap();assert_eq!(command.operation_id,failed["id"]);let failure=f.service.complete_runtime_failure(&f.agent,&command,"rollout_timeout").await.unwrap();assert_eq!(failure["status"],"needs-reconciliation");assert_eq!(f.service.complete_runtime_failure(&f.agent,&command,"rollout_timeout").await.unwrap(),failure);assert!(f.service.complete_runtime_failure(&f.agent,&command,"ownership_conflict").await.is_err());

 }).await;
    let docker = bollard::Docker::connect_with_unix(
        "/var/run/docker.sock",
        10,
        bollard::API_DEFAULT_VERSION,
    )
    .unwrap()
    .negotiate_version()
    .await
    .unwrap();
    let mut names = cleanup_names.lock().unwrap().clone();
    names.sort();
    names.dedup();
    for name in names {
        let _ = docker
            .remove_container(
                &name,
                Some(bollard::query_parameters::RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
        let _ = docker.remove_network(&format!("{name}-net")).await;
    }
    fixture.cleanup().await;
    outcome.unwrap();
}
#[tokio::test]
#[ignore = "requires explicit local retained-schema RLS fixture plus offline hosting migration"]
async fn retained_storage_requires_exact_delete_and_purge_proofs() {
    let fixture = Arc::new(Fixture::new("kubernetes").await);
    let f = fixture.clone();
    let outcome=tokio::spawn(async move{
  f.configure("k3s").await;let mut app_spec=spec();app_spec["volumeBytes"]=json!(67108864);
  let created=f.service.execute(&f.principal,json!({"action":"apps.create","projectID":f.pid,"name":"retained-storage","spec":app_spec,"subscriptionRevision":1,"idempotencyKey":"retained-create-request"})).await.unwrap();let app=created["appID"].as_str().unwrap();
  for(action,revision,deleted,volume,retained)in[("create",1,false,67108864,true),("apps.delete",2,true,67108864,true),("apps.purgeStorage",3,true,0,false)]{
   if action!="create"{let preview_action=if action=="apps.delete"{"apps.deletion"}else{"apps.storageDeletion"};let preview=f.service.execute(&f.principal,json!({"action":preview_action,"appID":app})).await.unwrap();f.service.execute(&f.principal,json!({"action":action,"appID":app,"confirmName":"retained-storage","planHash":preview["planHash"],"idempotencyKey":format!("retained-control-{revision}")})).await.unwrap();}
   let claim=f.service.agent_request(&f.agent,"claim",json!({})).await.unwrap();assert_eq!(claim["target_revision"],revision);
   let proof=json!({"appID":app,"revision":revision,"namespace":format!("wd-{}",f.pid),"uid":if deleted{Value::Null}else{json!("retained-uid")},"ready":true,"deleted":deleted,"replicas":if deleted{0}else{1},"storage":{"volumeBytes":volume,"retained":deleted&&retained}});
   let mut wrong=proof.clone();wrong["storage"]["retained"]=json!(!(deleted&&retained));assert!(f.service.agent_request(&f.agent,"complete",json!({"id":claim["id"],"generation":claim["generation"],"outcome":"succeeded","proof":wrong})).await.is_err());
   f.service.agent_request(&f.agent,"complete",json!({"id":claim["id"],"generation":claim["generation"],"outcome":"succeeded","proof":proof})).await.unwrap();
   let usage=f.service.execute(&f.principal,json!({"action":"usage.get","customerID":f.cid})).await.unwrap();assert_eq!(usage["allocated"]["volumeBytes"],volume);assert_eq!(usage["reserved"]["volumeBytes"],0);
  }
  assert_eq!(f.service.execute(&f.principal,json!({"action":"apps.storageDeletion","appID":app})).await.unwrap()["removed"],true);
 }).await;
    fixture.cleanup().await;
    outcome.unwrap();
}
#[cfg(feature = "docker-test")]
#[tokio::test]
#[ignore = "requires explicit WEBDOCK_HOSTING_DOCKER_TEST=1, native agent-control migrations and local Engine"]
async fn core_rpc_daemon_docker_receipts_survive_lost_poll_and_completion_ack() {
    use spitzli_agent_control::{ControlPlane, LocalBinding};
    use spitzli_agent_protocol::{self as wire, agent_control_client::AgentControlClient};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tonic::transport::{Endpoint, Server};
    use wire::agent_control_server::{AgentControl, AgentControlServer};
    struct LoseAck {
        core: ControlPlane,
        first: Arc<AtomicBool>,
        polls: Arc<AtomicUsize>,
        completions: Arc<AtomicUsize>,
    }
    #[tonic::async_trait]
    impl AgentControl for LoseAck {
        async fn register(
            &self,
            r: tonic::Request<wire::Hello>,
        ) -> Result<tonic::Response<wire::Welcome>, tonic::Status> {
            self.core.register(r).await
        }
        async fn heartbeat(
            &self,
            r: tonic::Request<wire::HeartbeatRequest>,
        ) -> Result<tonic::Response<wire::Ack>, tonic::Status> {
            self.core.heartbeat(r).await
        }
        async fn poll(
            &self,
            r: tonic::Request<wire::PollRequest>,
        ) -> Result<tonic::Response<wire::Dispatch>, tonic::Status> {
            self.polls.fetch_add(1, Ordering::SeqCst);
            self.core.poll(r).await
        }
        async fn guard(
            &self,
            r: tonic::Request<wire::GuardRequest>,
        ) -> Result<tonic::Response<wire::GuardReply>, tonic::Status> {
            self.core.guard(r).await
        }
        async fn complete(
            &self,
            r: tonic::Request<wire::Receipt>,
        ) -> Result<tonic::Response<wire::Ack>, tonic::Status> {
            self.completions.fetch_add(1, Ordering::SeqCst);
            let result = self.core.complete(r).await?;
            if self.first.swap(false, Ordering::SeqCst) {
                Err(tonic::Status::unavailable(
                    "injected lost completion acknowledgment",
                ))
            } else {
                Ok(result)
            }
        }
    }

    assert_eq!(
        std::env::var("WEBDOCK_HOSTING_DOCKER_TEST").as_deref(),
        Ok("1")
    );
    let engine = bollard::Docker::connect_with_unix(
        "/var/run/docker.sock",
        10,
        bollard::API_DEFAULT_VERSION,
    )
    .unwrap()
    .negotiate_version()
    .await
    .unwrap();
    engine
        .inspect_image(spec()["image"].as_str().unwrap())
        .await
        .expect("preload reviewed test image");
    let fixture = Arc::new(Fixture::new("docker").await);
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let state = directory.path().join("state");
    std::fs::create_dir(&state).unwrap();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket_path = directory.path().join("control.sock");
    let installation = fixture
        .owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT runtime_installation FROM webdock_auth.hosting_cluster WHERE id=$1",
            [fixture.cluster.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "runtime_installation")
        .unwrap();
    let uid = rustix::process::geteuid().as_raw();
    let hash = hex::encode(Sha256::digest("a".repeat(43)));
    sql(
        &fixture.owner,
        "UPDATE webdock_auth.hosting_agent SET last_sequence=0 WHERE cluster_id=$1",
        vec![fixture.cluster.clone().into()],
    )
    .await;
    sql(&fixture.owner,"INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid) VALUES($1,'hosting-fixture',1,$2,$3,'unix',$4)",vec![installation.clone().into(),fixture.cluster.clone().into(),hash.into(),i64::from(uid).into()]).await;
    let control = ControlPlane::new(
        db("AUTH_DATABASE_URL").await,
        fixture.service.clone(),
        Some(LocalBinding {
            installation: installation.clone(),
            agent_id: "hosting-fixture".into(),
            uid,
        }),
    )
    .await
    .unwrap();
    let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let polls = Arc::new(AtomicUsize::new(0));
    let completions = Arc::new(AtomicUsize::new(0));
    let lose_next_ack = Arc::new(AtomicBool::new(true));
    let proxy = LoseAck {
        core: control,
        first: lose_next_ack.clone(),
        polls: polls.clone(),
        completions: completions.clone(),
    };
    let server = tokio::spawn(async move {
        Server::builder()
            .add_service(
                AgentControlServer::new(proxy)
                    .max_decoding_message_size(128 * 1024)
                    .max_encoding_message_size(128 * 1024),
            )
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::UnixListenerStream::new(listener),
                async {
                    let _ = shutdown.await;
                },
            )
            .await
            .unwrap();
    });
    let f = fixture.clone();
    let captured_name = Arc::new(std::sync::Mutex::new(None::<String>));
    let cleanup_name = captured_name.clone();
    let test_installation = installation.clone();
    let observed_engine = engine.clone();
    let outcome=tokio::spawn(async move{
  f.configure("docker").await;let mut app_spec=spec();app_spec["args"]=json!(["sh","-c","echo rpc-log-fixture; printf ok >/tmp/index.html; exec httpd -f -p 8080 -h /tmp"]);app_spec["ephemeralBytes"]=json!(16777216);let created=f.service.execute(&f.principal,json!({"action":"apps.create","projectID":f.pid,"name":"rpc-fixture","spec":app_spec,"subscriptionRevision":1,"idempotencyKey":"rpc-create-request-1"})).await.unwrap();let app=created["appID"].as_str().unwrap();*captured_name.lock().unwrap()=Some(format!("wd-{test_installation}-{app}"));
  let channel=Endpoint::from_shared(format!("unix://{}",socket_path.display())).unwrap().connect().await.unwrap();let mut client=AgentControlClient::new(channel);let identity=wire::AgentIdentity{installation:test_installation.clone(),agent_id:"hosting-fixture".into(),generation:1};client.register(wire::Hello{identity:Some(identity.clone()),protocol_version:1,certificate_sha256:String::new(),capabilities_json:b"{}".to_vec()}).await.unwrap();
  let lost_poll=client.poll(wire::PollRequest{identity:Some(identity.clone())}).await.unwrap().into_inner().command.unwrap();assert_eq!(lost_poll.operation_id,created["id"]);
  let config=spitzli_agent::config::AgentConfig{database_runtime:None,mail_runtime:None,recovery_runtime:None,mail_configuration_enabled:false,workloads:false,plugin_runtime_enabled:false,version:1,installation:test_installation.clone(),agent_id:"hosting-fixture".into(),generation:1,state_directory:state.clone(),control:spitzli_agent::config::ControlEndpoint::Unix{path:socket_path},runtime:spitzli_agent::config::RuntimeConfiguration::Docker{storage_socket:None,socket:"/var/run/docker.sock".into(),initializer_image:None},policy:Default::default()};let mut daemon=spitzli_agent::management::Daemon::open(config).await.unwrap();daemon.connect().await.unwrap();assert!(daemon.cycle().await.is_err(),"simulate lost completion acknowledgment after Core commits");assert_eq!(completions.load(Ordering::SeqCst),1);
  let polls_before_reconnect=polls.load(Ordering::SeqCst);
  sql(&f.owner,"UPDATE webdock_auth.agent_runtime_dispatch SET receipt_sha256=NULL,completed_at=NULL WHERE operation_id=$1 AND generation=$2",vec![lost_poll.operation_id.clone().into(),(lost_poll.generation as i64).into()]).await;
  daemon.connect().await.unwrap();assert_eq!(completions.load(Ordering::SeqCst),2,"flush exact durable receipt before any new poll");assert_eq!(polls.load(Ordering::SeqCst),polls_before_reconnect);
  let view=f.service.execute(&f.principal,json!({"action":"apps.get","appID":app})).await.unwrap();assert_eq!(view["status"],"ready");
  let persisted:Value=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT command FROM webdock_auth.agent_runtime_dispatch WHERE operation_id=$1 AND generation=$2",[lost_poll.operation_id.clone().into(),(lost_poll.generation as i64).into()])).await.unwrap().unwrap().try_get("","command").unwrap();let command:spitzli_supervisor::Command=serde_json::from_value(persisted).unwrap();
  let journal:Value=serde_json::from_slice(&std::fs::read(state.join("commands.json")).unwrap()).unwrap();let observed:spitzli_supervisor::ObservedState=serde_json::from_value(journal["commands"][&command.operation_id]["result"].clone()).unwrap();
  let receipt=wire::Receipt{identity:Some(identity.clone()),operation_id:command.operation_id.clone(),generation:command.generation,request_sha256:command.fingerprint().unwrap(),outcome:wire::Outcome::Succeeded as i32,error_code:String::new(),result:Some(wire::receipt::Result::ObservedJson(serde_json::to_vec(&observed).unwrap()))};
  sql(&f.owner,"UPDATE webdock_auth.agent_runtime_dispatch SET receipt_sha256=NULL,completed_at=NULL WHERE operation_id=$1 AND generation=$2",vec![command.operation_id.clone().into(),(command.generation as i64).into()]).await;
  assert!(client.complete(receipt.clone()).await.unwrap().into_inner().accepted);assert!(client.complete(receipt.clone()).await.unwrap().into_inner().accepted);
  let mut changed=receipt.clone();changed.error_code="changed".into();assert!(client.complete(changed).await.is_err());
  let audit:i64=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT count(*) AS n FROM webdock_auth.hosting_audit WHERE subject=$1 AND action='apps.create.complete'",[f.uid.clone().into()])).await.unwrap().unwrap().try_get("","n").unwrap();assert_eq!(audit,1);

  for (action,key) in [("apps.logs","rpc-native-logs-request"),("apps.inspect","rpc-native-inspect-request"),("apps.metrics","rpc-native-metrics-request")]{
    let requested=f.service.execute(&f.principal,json!({"action":action,"appID":app,"idempotencyKey":key})).await.unwrap();
    if action=="apps.logs"{lose_next_ack.store(true,Ordering::SeqCst);assert!(daemon.cycle().await.is_err());let before=polls.load(Ordering::SeqCst);sql(&f.owner,"UPDATE webdock_auth.agent_runtime_dispatch SET receipt_sha256=NULL,completed_at=NULL WHERE operation_id=$1",vec![requested["id"].as_str().unwrap().into()]).await;daemon.connect().await.unwrap();assert_eq!(polls.load(Ordering::SeqCst),before);}else{daemon.cycle().await.unwrap();}
    let result=serde_json::to_value(f.service.get_read(&f.principal,requested["id"].as_str().unwrap()).await.unwrap()).unwrap();assert_eq!(result["status"],"succeeded","{}",result);
    match action{"apps.logs"=>assert!(result["result"]["logs"].as_str().unwrap().contains("rpc-log-fixture")),"apps.inspect"=>assert_eq!(result["result"]["resourceId"],app),_=>assert!(result["result"]["memoryBytes"].as_u64().is_some())};
  }

  let snapshot=f.service.execute(&f.principal,json!({"action":"clusters.get","clusterID":f.cluster})).await.unwrap();assert_eq!(snapshot["observation"]["capabilities"]["environment"],true,"actual daemon capability");
  for (revision,value) in [(1,Some("core-private-env-one")),(2,Some("core-private-env-two")),(3,None)]{
    let operation=f.service.execute(&f.principal,json!({"action":"apps.update","appID":app,"revision":revision,"spec":app_spec,"environment":[{"name":"TOKEN","value":value}],"subscriptionRevision":1,"idempotencyKey":format!("rpc-env-update-{revision}")})).await.unwrap();daemon.cycle().await.unwrap();
    let name=captured_name.lock().unwrap().clone().unwrap();let actual=observed_engine.inspect_container(&name,None).await.unwrap().config.unwrap().env.unwrap_or_default();if let Some(value)=value{assert!(actual.contains(&format!("TOKEN={value}")));}else{assert!(!actual.iter().any(|v|v.starts_with("TOKEN=")));}
    let stored:Value=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT command AS value FROM webdock_auth.agent_runtime_dispatch WHERE operation_id=$1",[operation["id"].as_str().unwrap().into()])).await.unwrap().unwrap().try_get("","value").unwrap();assert!(!stored.to_string().contains("core-private-env"));
  }

  let deletion=f.service.execute(&f.principal,json!({"action":"apps.deletion","appID":app})).await.unwrap();
  let retired=f.service.execute(&f.principal,json!({"action":"apps.delete","appID":app,"planHash":deletion["planHash"],"confirmName":"rpc-fixture","idempotencyKey":"rpc-retire-request-1"})).await.unwrap();daemon.cycle().await.unwrap();let retired_state=f.service.execute(&f.principal,json!({"action":"operations.get","operationID":retired["id"]})).await.unwrap();assert_eq!(retired_state["status"],"succeeded");let name=captured_name.lock().unwrap().clone().unwrap();assert!(observed_engine.inspect_container(&name,None).await.is_err());
  sql(&f.owner,"UPDATE webdock_auth.agent_transport_enrollment SET revoked=true WHERE installation=$1 AND agent_id='hosting-fixture'",vec![test_installation.clone().into()]).await;assert!(client.complete(receipt).await.is_err());
 }).await;
    let _ = stop.send(());
    server.await.unwrap();
    let name = cleanup_name.lock().unwrap().clone();
    if let Some(name) = name {
        let _ = engine
            .remove_container(
                &name,
                Some(bollard::query_parameters::RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
        let _ = engine.remove_network(&format!("{name}-net")).await;
    }
    sql(&fixture.owner,"DELETE FROM webdock_auth.agent_runtime_dispatch WHERE installation=$1 AND agent_id='hosting-fixture'",vec![installation.clone().into()]).await;
    sql(&fixture.owner,"DELETE FROM webdock_auth.agent_transport_enrollment WHERE installation=$1 AND agent_id='hosting-fixture'",vec![installation.into()]).await;
    fixture.cleanup().await;
    outcome.unwrap();
}

#[path = "../../build/tests/support/hosting_flow.rs"]
mod git_build_flow;
#[cfg(feature = "docker-test")]
#[path = "support/native_browser.rs"]
mod native_browser;
#[path = "support/read_http.rs"]
mod read_http;
#[tokio::test]
#[ignore = "owned local restricted-role hosting fixture"]
async fn native_reads_preserve_leases_ownership_redaction_and_exact_receipts() {
    let fixture = Arc::new(Fixture::new("kubernetes").await);
    let f = fixture.clone();
    let outcome=tokio::spawn(async move {
 f.configure("k3s").await;
 let created=f.service.execute(&f.principal,json!({"action":"apps.create","projectID":f.pid,"name":"read-fixture","spec":spec(),"environment":[{"name":"TOKEN","value":"prior-secret-value"}],"subscriptionRevision":1,"idempotencyKey":"native-read-create-1"})).await.unwrap();let app=created["appID"].as_str().unwrap();let initial=f.service.agent_request(&f.agent,"claim",json!({})).await.unwrap();f.proof(&initial,"read-owned-uid").await;
 f.service.execute(&f.principal,json!({"action":"apps.update","appID":app,"revision":1,"spec":spec(),"environment":[{"name":"TOKEN","value":"current-secret-value"}],"subscriptionRevision":1,"idempotencyKey":"native-read-update-1"})).await.unwrap();let update=f.service.agent_request(&f.agent,"claim",json!({})).await.unwrap();f.proof(&update,"read-owned-uid").await;
 let request=f.service.execute(&f.principal,json!({"action":"apps.logs","appID":app,"idempotencyKey":"native-read-logs-1"})).await.unwrap();let command=f.service.claim_read(&f.agent).await.unwrap().unwrap();assert_eq!(command.operation_id,request["id"].as_str().unwrap());assert!(command.spec.hosting_binding.is_some());assert_eq!(command.expected_runtime_id.as_deref(),Some("read-owned-uid"));f.service.guard_read(&f.agent,&command).await.unwrap();let mut changed=command.clone();changed.expected_runtime_id=Some("other-uid".into());assert!(f.service.guard_read(&f.agent,&changed).await.is_err());
 let result=json!({"logs":"old=prior-secret-value current=current-secret-value safe","truncated":false});f.service.complete_read(&f.agent,&command,result.clone()).await.unwrap();f.service.complete_read(&f.agent,&command,result).await.unwrap();assert!(f.service.complete_read(&f.agent,&command,json!({"logs":"changed","truncated":false})).await.is_err());let view=serde_json::to_value(f.service.get_read(&f.principal,&command.operation_id).await.unwrap()).unwrap();assert_eq!(view["status"],"succeeded");assert_eq!(view["result"]["logs"],"old=[redacted] current=[redacted] safe");
 let persisted:Value=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT result AS value FROM webdock_auth.hosting_operation WHERE id=$1",[command.operation_id.clone().into()])).await.unwrap().unwrap().try_get("","value").unwrap();assert!(!persisted.to_string().contains("prior-secret-value"));assert!(!persisted.to_string().contains("current-secret-value"));
 for (action,key) in [("apps.inspect","native-read-inspect-1"),("apps.metrics","native-read-metrics-1")]{f.service.execute(&f.principal,json!({"action":action,"appID":app,"idempotencyKey":key})).await.unwrap();let read=f.service.claim_read(&f.agent).await.unwrap().unwrap();let result=if action=="apps.inspect"{json!({"resourceId":app,"revision":2,"phase":"running","ready":true,"observedAt":chrono::Utc::now().timestamp(),"runtimeId":"read-owned-uid","conditions":[]})}else{json!({"observedAt":chrono::Utc::now().timestamp(),"cpuNanoseconds":null,"memoryBytes":12345})};f.service.complete_read(&f.agent,&read,result).await.unwrap();}
 f.service.execute(&f.principal,json!({"action":"apps.logs","appID":app,"idempotencyKey":"native-read-failure-1"})).await.unwrap();let failure=f.service.claim_read(&f.agent).await.unwrap().unwrap();f.service.complete_read_failure(&f.agent,&failure,"runtime_unavailable").await.unwrap();f.service.complete_read_failure(&f.agent,&failure,"runtime_unavailable").await.unwrap();assert!(f.service.complete_read_failure(&f.agent,&failure,"different_failure").await.is_err());
 f.service.execute(&f.principal,json!({"action":"apps.logs","appID":app,"idempotencyKey":"native-read-revoke-1"})).await.unwrap();let revoked=f.service.claim_read(&f.agent).await.unwrap().unwrap();sql(&f.owner,"UPDATE webdock_auth.\"user\" SET banned=true WHERE id=$1",vec![f.uid.clone().into()]).await;assert!(f.service.guard_read(&f.agent,&revoked).await.is_err());assert!(f.service.complete_read(&f.agent,&revoked,json!({"logs":"not-authorized","truncated":false})).await.is_err());
 }).await;
    fixture.cleanup().await;
    outcome.unwrap();
}
#[tokio::test]
#[ignore = "owned local restricted role fixture"]
async fn runtime_material_is_bound_to_persisted_ciphertext_and_never_serialized() {
    let fixture = Arc::new(Fixture::new("docker").await);
    let f = fixture.clone();
    let outcome=tokio::spawn(async move{
 f.configure("docker").await;sql(&f.owner,"UPDATE webdock_auth.hosting_agent SET observation=jsonb_set(observation,'{capabilities,environment}','true') WHERE cluster_id=$1",vec![f.cluster.clone().into()]).await;
 let created=f.service.execute(&f.principal,json!({"action":"apps.create","projectID":f.pid,"name":"material-fixture","spec":spec(),"environment":[{"name":"TOKEN","value":"immutable-operation-secret"}],"subscriptionRevision":1,"idempotencyKey":"material-create-request"})).await.unwrap();let mut command=f.service.claim_runtime(&f.agent).await.unwrap().unwrap();assert_eq!(command.operation_id,created["id"].as_str().unwrap());assert!(command.spec.hosted_access.is_some());
 sql(&f.owner,"UPDATE webdock_auth.hosting_app SET environment_encrypted=NULL WHERE id=$1",vec![created["appID"].as_str().unwrap().into()]).await;
 let material=f.service.runtime_material(&f.agent,&command).await.unwrap().unwrap();assert_eq!(material.environment.as_ref().unwrap().values["TOKEN"],"immutable-operation-secret");command.material=Some(material);command.verify_material().unwrap();assert!(!serde_json::to_string(&command).unwrap().contains("immutable-operation-secret"));
 let mut tampered=command.clone();tampered.spec.hosted_access.as_mut().unwrap().environment.as_mut().unwrap().source_sha256="0".repeat(64);assert!(f.service.runtime_material(&f.agent,&tampered).await.is_err());
 sql(&f.owner,"UPDATE webdock_auth.hosting_agent SET revoked=true WHERE cluster_id=$1",vec![f.cluster.clone().into()]).await;assert!(f.service.runtime_material(&f.agent,&command).await.is_err());
 }).await;
    fixture.cleanup().await;
    outcome.unwrap();
}

#[path = "../../build/tests/support/repository_flow.rs"]
mod repository_flow;

#[path = "../../build/tests/support/core_flow.rs"]
mod build_core_flow;

#[path = "../../build/tests/support/browser.rs"]
mod browser;

#[path = "support/delivery.rs"]
mod delivery;

#[path = "support/completion_deadline.rs"]
mod completion_deadline;

#[path = "../../build/tests/support/kubernetes_admission.rs"]
mod build_kubernetes_admission;
#[path = "../../build/tests/support/kubernetes_fair_retry.rs"]
mod build_kubernetes_fair_retry;
#[path = "../../build/tests/support/kubernetes_volumes.rs"]
mod build_kubernetes_volumes;

#[path = "../../build/tests/support/kubernetes_publication.rs"]
mod build_kubernetes_publication;

#[path = "../../build/tests/support/github_webhook.rs"]
mod build_github_webhook;

#[path = "../../build/tests/support/kubernetes_live.rs"]
mod build_kubernetes_live;
