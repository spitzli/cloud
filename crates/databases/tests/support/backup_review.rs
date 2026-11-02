//! Fresh Core/RLS metadata regressions. No controller principal or physical Ready is fabricated.
//! Initial SQL rows model existing installations; connected acceptance separately proves real CNPG.
use super::support::{exec, identity, row};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use webdock_agent_protocol::database::*;
use webdock_databases::{DatabaseService, StoredAuthority};
use webdock_identity::{Principal, RlsSigner};
use webdock_registry::RegistryService;
struct Fixture {
    owner: DatabaseConnection,
    service: DatabaseService,
    hosting: webdock_hosting::HostingService,
    registry: RegistryService,
    registry_db: DatabaseConnection,
    actor: Principal,
    ids: Arc<webdock_persistence::IdAllocator>,
    cluster: String,
    customer: String,
    project: String,
    database: String,
    backend: String,
    policy: String,
    agent_key: String,
}
impl Fixture {
    async fn open() -> Self {
        Self::setup(true).await
    }
    async fn setup(grant: bool) -> Self {
        assert_eq!(
            std::env::var("WEBDOCK_BACKUP_TRANSITION_TEST").as_deref(),
            Ok("yes")
        );
        let connect = |key| Database::connect(std::env::var(key).unwrap());
        let owner = connect("WEBDOCK_TEST_MIGRATION_DATABASE_URL")
            .await
            .unwrap();
        let auth = connect("AUTH_DATABASE_URL").await.unwrap();
        let business = connect("DATABASE_URL").await.unwrap();
        let worker = connect("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await.unwrap();
        let ids = Arc::new(
            webdock_persistence::IdAllocator::connect(
                &std::env::var("AUTH_DATABASE_URL").unwrap(),
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )
            .await
            .unwrap(),
        );
        let (identity, actor) = identity(auth.clone(), ids.clone()).await;
        let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
        let rls = std::env::var("DATABASE_RLS_SECRET").unwrap();
        let cluster = ids.allocate().await.unwrap().to_string();
        let customer = ids.allocate().await.unwrap().to_string();
        let project = ids.allocate().await.unwrap().to_string();
        exec(&owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('databases','builtin','enabled','enabled','review') ON CONFLICT DO NOTHING",vec![]).await;
        exec(&owner,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,'Review operator',$2,'operator',$3)",vec![ids.allocate().await.unwrap().to_string().into(),actor.email.clone().into(),actor.user_id.clone().into()]).await;
        exec(&owner,"WITH hint AS MATERIALIZED(SELECT set_config('webdock.organization_allocation',$2,true)) INSERT INTO webdock_admin.customers(id,name) SELECT $1,'Backup review' FROM hint",vec![customer.clone().into(),json!({"customer_id":customer,"organization_id":ids.allocate().await.unwrap().to_string()}).to_string().into()]).await;
        exec(
            &owner,
            "INSERT INTO webdock_admin.projects(id,customer_id,name) VALUES($1,$2,'Backup review')",
            vec![project.clone().into(), customer.clone().into()],
        )
        .await;
        let capacity = json!({"apps":20,"cpuMillicores":16000,"memoryBytes":34359738368_u64,"volumeBytes":68719476736_u64,"ephemeralBytes":17179869184_u64,"replicasPerApp":1,"concurrentDeployments":10});
        exec(&owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,verified,capacity,runtime_kind,runtime_installation) VALUES($1,'Metadata-only review fixture','fixture','local','unit metadata; no physical readiness claim',true,$2,'kubernetes','backup-review')",vec![cluster.clone().into(),capacity.clone().into()]).await;
        let pool = "00000000-0000-4000-8000-000000000005";
        let observation = json!({"capacity":capacity,"nodes":[{"id":"fixture-node","cpuMillicores":16000,"memoryBytes":34359738368_u64}],"runtimeKind":"kubernetes","capabilities":{"networkIsolation":true,"restrictedPods":true,"storage":false,"databaseVersion":1,"databaseBackupVersion":1,"databaseNodePorts":[],"databasePools":[{"node":"fixture-node","poolID":pool,"ready":true,"capacityBytes":34359738368_u64,"availableBytes":34359738368_u64,"allocatedBytes":0,"requestedCpuMillicores":0,"requestedMemoryBytes":0,"allocations":[],"observedDatabases":[]}]}});
        let mut random = [0u8; 32];
        getrandom::getrandom(&mut random).unwrap();
        let agent_key = URL_SAFE_NO_PAD.encode(random);
        let credential_hash = hex::encode(Sha256::digest(&agent_key));
        exec(&owner,"INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_sequence,last_seen,observation) VALUES($1,$2,1,0,now(),$3)",vec![cluster.clone().into(),credential_hash.into(),observation.into()]).await;
        exec(&owner,"INSERT INTO webdock_auth.hosting_database_pool(cluster_id,node,pool_id,capacity_bytes) VALUES($1,'fixture-node',$2::uuid,34359738368)",vec![cluster.clone().into(),pool.into()]).await;
        let hosting = webdock_hosting::HostingService::new(
            business.clone(),
            auth.clone(),
            worker.clone(),
            rls.clone(),
            secret.clone(),
            ids.clone(),
        )
        .await
        .unwrap();
        let service = DatabaseService::new(
            business,
            auth,
            worker,
            identity,
            None,
            ids.clone(),
            rls.clone(),
            &secret,
            "http://127.0.0.1:3150",
        )
        .await
        .unwrap()
        .with_management(webdock_databases::ManagementConfig {
            cluster_id: Some(cluster.clone()),
            endpoint_domain: Some("fixture.invalid".into()),
            public_ordering: false,
            protected_database_id: None,
            lease_secret: Some(secret.clone()),
        })
        .unwrap();
        let registry_db = connect("WEBDOCK_REGISTRY_DATABASE_URL").await.unwrap();
        let registry = RegistryService::new(
            registry_db.clone(),
            RlsSigner::new(rls, "webdock_registry_runtime").unwrap(),
        )
        .with_allocator(ids.clone());
        // Seed a model installation once, before testing transitions. Never install a
        // synthetic agent completion or repair rows after a tested operation fails.
        let plan_id = ids.allocate().await.unwrap().to_string();
        let offer_id = ids.allocate().await.unwrap().to_string();
        let order_id = ids.allocate().await.unwrap().to_string();
        let database = ids.allocate().await.unwrap().to_string();
        let backend = ids.allocate().await.unwrap().to_string();
        let plan = json!({"kind":"dedicated","postgresMajor":18,"cpuMillicores":500,"memoryBytes":1073741824,"volumeBytes":1610612736,"logicalStorageAllowanceBytes":null,"name":"Review model","monthlyPriceCents":1200,"currency":"EUR","connectionLimit":10,"terms":"Core metadata test model; no physical readiness assertion or production service."});
        exec(&owner,"INSERT INTO webdock_auth.hosting_database_plan(id,plan,availability) VALUES($1,$2,'pilot')",vec![plan_id.clone().into(),plan.clone().into()]).await;
        exec(&owner,"INSERT INTO webdock_auth.hosting_database_offer(id,customer_id,project_id,plan_revision_id,name,snapshot) VALUES($1,$2,$3,$4,'Source',$5)",vec![offer_id.clone().into(),customer.clone().into(),project.clone().into(),plan_id.clone().into(),plan.clone().into()]).await;
        exec(&owner,"INSERT INTO webdock_auth.hosting_database_order(id,offer_id,customer_id,project_id,plan_revision_id,accepted_by,snapshot) VALUES($1,$2,$3,$4,$5,$6,$7)",vec![order_id.clone().into(),offer_id.into(),customer.clone().into(),project.clone().into(),plan_id.clone().into(),actor.user_id.clone().into(),plan.into()]).await;
        let profile =
            json!({"cpuMillicores":500,"memoryBytes":1073741824,"volumeBytes":1610612736});
        let demand = json!({"apps":2,"cpuMillicores":700,"memoryBytes":1610612736,"volumeBytes":1610612736,"ephemeralBytes":536870912,"replicasPerApp":1,"concurrentDeployments":1});
        exec(&owner,"INSERT INTO webdock_auth.hosting_database_backend(id,cluster_id,kind,node,pool_id,state,cnpg_uid,demand,profile,max_databases,logical_capacity_bytes,host,direct_port,pooled_port) VALUES($1,$2,'dedicated','fixture-node',$3::uuid,'ready',$4,$5,$6,1,0,$7,32000,32001)",vec![backend.clone().into(),cluster.clone().into(),pool.into(),format!("metadata-only-{backend}").into(),demand.into(),profile.into(),format!("db-{backend}.fixture.invalid").into()]).await;
        let credentials = DatabaseCredentials {
            application_password: URL_SAFE_NO_PAD.encode([17u8; 32]),
            migration_password: URL_SAFE_NO_PAD.encode([19u8; 32]),
        };
        let sealed = webdock_secrets::SecretCipher::new(&secret)
            .unwrap()
            .seal(&credentials, &format!("database:{database}:1"))
            .unwrap();
        exec(&owner,"INSERT INTO webdock_auth.hosting_database(id,customer_id,project_id,order_id,plan_revision_id,backend_id,kind,name,database_name,state,credentials_encrypted,confirmed_credential_revision,confirmed_credentials_encrypted) VALUES($1,$2,$3,$4,$5,$6,'dedicated','Source',$7,'ready',$8,1,$8)",vec![database.clone().into(),customer.clone().into(),project.clone().into(),order_id.into(),plan_id.into(),backend.clone().into(),format!("db_{database}").into(),sealed.into()]).await;
        let mut f = Self {
            owner,
            service,
            hosting,
            registry,
            registry_db,
            actor,
            ids,
            cluster,
            customer,
            project,
            database,
            backend,
            policy: String::new(),
            agent_key,
        };
        if !grant {
            return f;
        }
        let destination=f.command(json!({"action":"databases.backupDestinations.register","endpointURL":"https://review.fixture.invalid","region":"local","bucket":"review-fixture","keyRecoveryID":"110","writeCredentials":{"accessKeyId":"review-write","secretAccessKey":"fixture-only-writer-material","readOnly":false},"recoveryCredentials":{"accessKeyId":"review-read","secretAccessKey":"fixture-only-reader-material","readOnly":true},"idempotencyKey":"review-destination-registration"})).await;
        let granted=f.command(json!({"action":"databases.backupPolicies.grant","databaseID":f.database,"revision":1,"destinationID":destination["destinationID"],"schedule":"0 0 1 * * *","retentionDays":30,"retentionDelete":false,"idempotencyKey":"review-policy-grant"})).await;
        f.policy = granted["policyID"].as_str().unwrap().into();
        f
    }
    async fn command(&self, command: Value) -> Value {
        self.service
            .execute_managed(&self.actor, &StoredAuthority::LocalSession {}, command)
            .await
            .unwrap()
    }
    async fn policy_update(
        &self,
        revision: i32,
        suspended: bool,
        wal: bool,
        retention: bool,
    ) -> Value {
        self.command(json!({"action":"databases.backupPolicies.update","databaseID":self.database,"revision":1,"policyRevision":revision,"schedule":"0 0 1 * * *","suspended":suspended,"walEnabled":wal,"retentionDelete":retention,"retentionDays":30,"idempotencyKey":format!("review-policy-update-{revision}")})).await
    }
}
#[tokio::test]
#[ignore = "fresh owned Core metadata fixture only"]
async fn invalid_legacy_queued_backup_does_not_block_the_next_valid_job_or_release_capacity() {
    let f = Fixture::open().await;
    let valid = f.policy_update(1, true, true, false).await;
    assert!(f.service.execute_managed(&f.actor,&StoredAuthority::LocalSession{},json!({"action":"databases.backup","databaseID":f.database,"revision":1,"idempotencyKey":"review-suspended-request"})).await.is_err());
    let invalid = f.ids.allocate().await.unwrap().to_string();
    exec(&f.owner,"INSERT INTO webdock_auth.hosting_operation(id,subject,actor_session,customer_id,project_id,cluster_id,action,idempotency_key,payload_hash,status,desired,target_revision,created_at) VALUES($1,$2,$3,$4,$5,$6,'databases.backup',$7,$8,'queued','{}',1,now()-interval '1 minute')",vec![invalid.clone().into(),f.actor.user_id.clone().into(),f.actor.session_id.clone().into(),f.customer.clone().into(),f.project.clone().into(),f.cluster.clone().into(),format!("legacy-invalid-{invalid}").into(),"a".repeat(64).into()]).await;
    exec(&f.owner,"INSERT INTO webdock_auth.hosting_database_job(operation_id,backend_id,database_id,action,revision,backup_policy_id,backup_policy_revision) VALUES($1,$2,$3,'backup',1,$4,2)",vec![invalid.clone().into(),f.backend.clone().into(),f.database.clone().into(),f.policy.clone().into()]).await;
    let held = row(
        &f.owner,
        "SELECT demand AS value FROM webdock_auth.hosting_database_backend WHERE id=$1",
        &f.backend,
    )
    .await;
    let agent = f
        .hosting
        .authenticate_agent(&f.cluster, &f.agent_key)
        .await
        .unwrap();
    let first = f
        .service
        .claim_database_operation(
            f.hosting.begin_authenticated_agent(&agent).await.unwrap(),
            &agent,
        )
        .await
        .expect("invalid unissued job must be retired in a committed transaction");
    assert!(first.is_none());
    let next = f
        .service
        .claim_database_operation(
            f.hosting.begin_authenticated_agent(&agent).await.unwrap(),
            &agent,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.spec.operation_id, valid["operationID"]);
    next.spec.validate().unwrap();
    assert_eq!(
        held,
        row(
            &f.owner,
            "SELECT demand AS value FROM webdock_auth.hosting_database_backend WHERE id=$1",
            &f.backend
        )
        .await
    );
    let retired=row(&f.owner,"SELECT jsonb_build_object('status',o.status,'lease',o.lease_until,'issued',o.desired->'nativeIssuedSpecs','checkpoint',j.checkpoint) AS value FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_database_job j ON j.operation_id=o.id WHERE o.id=$1",&invalid).await;
    assert_eq!(retired["status"], "failed");
    assert!(
        retired["lease"].is_null()
            && retired["issued"].is_null()
            && retired["checkpoint"].is_null()
    );
}
#[tokio::test]
#[ignore = "fresh owned Core/Registry fixture only"]
async fn actual_registry_archive_paths_wait_for_observed_backup_stop() {
    let f = Fixture::open().await;
    let blocked = f
        .registry
        .archive(&f.actor, "projects", &f.project, true)
        .await;
    assert!(
        blocked.is_err(),
        "active autonomous policy must block actual Registry project archival"
    );
    f.policy_update(1, true, false, false).await;
    assert!(
        f.registry
            .archive(&f.actor, "projects", &f.project, true)
            .await
            .is_err(),
        "desired stopped flags without controller observation cannot authorize archival"
    );
    assert!(
        f.registry
            .archive(&f.actor, "customers", &f.customer, true)
            .await
            .is_err()
    );
    let signer = RlsSigner::new(
        std::env::var("DATABASE_RLS_SECRET").unwrap(),
        "webdock_registry_runtime",
    )
    .unwrap();
    let tx = signer.begin(&f.registry_db, &f.actor).await.unwrap();
    assert!(
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
            [f.customer.clone().into()]
        ))
        .await
        .is_err(),
        "customer guard also protects direct/cascading status writes"
    );
    let _ = tx.rollback().await;
    let parent = row(
        &f.owner,
        "SELECT to_jsonb(status) AS value FROM webdock_admin.customers WHERE id=$1",
        &f.customer,
    )
    .await;
    assert_eq!(parent, "active");
}
#[tokio::test]
#[ignore = "fresh owned Core metadata fixture only"]
async fn resize_keeps_policy_resource_revision_and_reserved_peak_aligned() {
    let f = Fixture::open().await;
    f.policy_update(1, false, true, true).await;
    let before = row(
        &f.owner,
        "SELECT spec AS value FROM webdock_auth.hosting_database_backup_policy WHERE id=$1",
        &f.policy,
    )
    .await;
    let mut plan=row(&f.owner,"SELECT p.plan AS value FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_plan p ON p.id=d.plan_revision_id WHERE d.id=$1",&f.database).await;
    plan["cpuMillicores"] = json!(1000);
    let plan=f.command(json!({"action":"databases.plans.create","plan":plan,"idempotencyKey":"review-resize-plan-create"})).await;
    f.command(json!({"action":"databases.plans.setAvailability","planRevisionID":plan["id"],"availability":"pilot","revision":1,"idempotencyKey":"review-resize-plan-available"})).await;
    let offer=f.command(json!({"action":"databases.offers.create","databaseID":f.database,"projectID":f.project,"planRevisionID":plan["id"],"name":"Resize","idempotencyKey":"review-resize-offer"})).await;
    f.command(json!({"action":"databases.acceptOrder","projectID":f.project,"offerID":offer["id"],"confirmInterruption":true,"idempotencyKey":"review-resize-accept"})).await;
    let backend = row(
        &f.owner,
        "SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1",
        &f.backend,
    )
    .await;
    let policy=row(&f.owner,"SELECT to_jsonb(p) AS value FROM webdock_auth.hosting_database_backup_policy p WHERE id=$1",&f.policy).await;
    assert_eq!(
        policy["spec"]["resourceRevision"], backend["revision"],
        "resize must not leave backup authority bound to an obsolete resource revision"
    );
    assert_eq!(policy["spec"]["walEnabled"], before["walEnabled"]);
    assert_eq!(policy["spec"]["retentionDelete"], before["retentionDelete"]);
    assert_eq!(policy["spec"]["suspended"], before["suspended"]);
    assert_eq!(backend["demand"]["cpuMillicores"], 1800);
    assert_eq!(backend["demand"]["memoryBytes"], 2415919104_u64);
    assert_eq!(backend["demand"]["ephemeralBytes"], 1342177280);
    assert_ne!(
        policy["revision"], policy["observed_revision"],
        "resized configuration must be reobserved before configured status"
    );
}

#[tokio::test]
#[ignore = "fresh owned Core/Registry fixture only"]
async fn grant_cannot_commit_after_registry_archival_wins_the_scope_lock() {
    let f = Fixture::setup(false).await;
    let destination=f.command(json!({"action":"databases.backupDestinations.register","endpointURL":"https://review.fixture.invalid","region":"local","bucket":"review-fixture","keyRecoveryID":"110","writeCredentials":{"accessKeyId":"review-write","secretAccessKey":"fixture-only-writer-material","readOnly":false},"recoveryCredentials":{"accessKeyId":"review-read","secretAccessKey":"fixture-only-reader-material","readOnly":true},"idempotencyKey":"review-race-destination"})).await;
    let signer = RlsSigner::new(
        std::env::var("DATABASE_RLS_SECRET").unwrap(),
        "webdock_registry_runtime",
    )
    .unwrap();
    let tx = signer.begin(&f.registry_db, &f.actor).await.unwrap();
    tx.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM webdock_admin.projects WHERE id=$1 FOR UPDATE",
        [f.project.clone().into()],
    ))
    .await
    .unwrap();
    let local = StoredAuthority::LocalSession {};
    let grant=f.service.execute_managed(&f.actor,&local,json!({"action":"databases.backupPolicies.grant","databaseID":f.database,"revision":1,"destinationID":destination["destinationID"],"schedule":"0 0 1 * * *","retentionDays":30,"retentionDelete":false,"idempotencyKey":"review-racing-grant"}));
    let archive = async {
        let mut waiting = false;
        for _ in 0..200 {
            let count=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*)::bigint AS n FROM pg_stat_activity WHERE usename='webdock_auth_runtime' AND wait_event_type='Lock'".to_owned())).await.unwrap().unwrap().try_get::<i64>("","n").unwrap();
            if count > 0 {
                waiting = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            waiting,
            "the real grant must wait on Registry's held project lock"
        );
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_admin.projects SET status='archived' WHERE id=$1",
            [f.project.clone().into()],
        ))
        .await
        .unwrap();
        tx.commit().await.unwrap();
    };
    let (grant, ()) = tokio::join!(grant, archive);
    assert!(grant.is_err());
    assert!(row(&f.owner,"SELECT to_jsonb(count(*)::integer) AS value FROM webdock_auth.hosting_database_backup_policy WHERE database_id=$1",&f.database).await==0);
}
#[tokio::test]
#[ignore = "real owned connected fixture only"]
async fn live_registry_archive_obeys_actual_native_backup_observation() {
    assert_eq!(
        std::env::var("WEBDOCK_BACKUP_LIVE_REGISTRY").as_deref(),
        Ok("yes")
    );
    let source: Value =
        serde_json::from_slice(&std::fs::read("/connected/review-source.json").unwrap()).unwrap();
    let owner = Database::connect(std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let auth = Database::connect(std::env::var("AUTH_DATABASE_URL").unwrap())
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
    let (_, actor) = identity(auth, ids.clone()).await;
    exec(&owner,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,'Live review operator',$2,'operator',$3)",vec![ids.allocate().await.unwrap().to_string().into(),actor.email.clone().into(),actor.user_id.clone().into()]).await;
    let db = Database::connect(std::env::var("WEBDOCK_REGISTRY_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let service = RegistryService::new(
        db,
        RlsSigner::new(
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            "webdock_registry_runtime",
        )
        .unwrap(),
    )
    .with_allocator(ids);
    let project = source["project"].as_str().unwrap();
    let customer = source["customer"].as_str().unwrap();
    let expected = std::env::var("WEBDOCK_BACKUP_ARCHIVE_EXPECT").unwrap();
    let result = service.archive(&actor, "projects", project, true).await;
    if expected == "blocked" {
        assert_eq!(
            result.unwrap_err().status(),
            409,
            "actual project archive must wait for native stopped evidence"
        );
        assert!(
            service
                .archive(&actor, "customers", customer, true)
                .await
                .is_err()
        );
        assert_eq!(
            row(
                &owner,
                "SELECT to_jsonb(status) AS value FROM webdock_admin.projects WHERE id=$1",
                project
            )
            .await,
            "active"
        );
    } else {
        assert_eq!(expected, "stopped");
        result.unwrap();
        service
            .archive(&actor, "customers", customer, true)
            .await
            .unwrap();
        let defaults=row(&owner,"SELECT jsonb_agg(to_jsonb(status)) AS value FROM webdock_admin.projects WHERE customer_id=$1 AND created_default",customer).await;
        assert_eq!(
            defaults,
            json!(["archived"]),
            "actual Registry customer archive cascades its empty Default project"
        );
    }
    let path = "/connected/evidence.json";
    let mut e: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    e[format!("registryArchive-{expected}")] = json!({"actualRegistryAPI":true,"project":if expected=="blocked"{"refused-409"}else{"archived"},"customer":if expected=="blocked"{"refused"}else{"archived-with-default-cascade"},"noSyntheticPrincipal":true});
    std::fs::write(path, serde_json::to_vec_pretty(&e).unwrap()).unwrap();
}
