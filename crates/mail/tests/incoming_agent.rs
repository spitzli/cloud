//! Restricted database authority/receipt checks. Runtime proofs here are controlled inputs;
//! the separate real-engine acceptance must establish external lifecycle behavior.
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::FutureExt;
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TransactionTrait,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use webdock_agent_protocol::mail::*;
use webdock_browser_session::StoredAuthority;
use webdock_identity::{CookiePolicy, IdentityService};
use webdock_mail::{
    MailDependencies,
    incoming::{
        IncomingConfig, NativeIncoming, ServiceCommand, ServiceState,
        agent::{IncomingAgent, MailAgentConfig},
    },
};
const KEY: &str = "owned-incoming-agent-context-key-atleast32";
const IMAGE: &str = "registry.example.test/stalwart@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
async fn sql(db: &impl ConnectionTrait, query: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        query,
        values,
    ))
    .await
    .unwrap();
}
async fn role(key: &str, name: &str, host: &url::Url) -> DatabaseConnection {
    let mut url = url::Url::parse(&std::env::var(key).unwrap()).unwrap();
    assert_eq!(url.host_str(), host.host_str());
    assert_eq!(url.port(), host.port());
    url.set_path(&format!("/{name}"));
    Database::connect(url.as_str()).await.unwrap()
}
#[tokio::test]
#[ignore = "creates and removes an owned local Mail agent database"]
async fn native_mail_claim_checkpoint_receipt_are_fenced_and_replayed_exactly() {
    fixture(Case::Domain).await;
}
#[tokio::test]
#[ignore = "creates and removes an owned local Mail lock-order database"]
async fn customer_fence_precedes_service_and_cluster_locks_without_cancellation() {
    fixture(Case::LockOrder).await;
}
#[tokio::test]
#[ignore = "creates and removes an owned local Mail authority-failure database"]
async fn authority_storage_failure_does_not_retire_or_acknowledge_work() {
    fixture(Case::StorageFailure).await;
}
#[tokio::test]
#[ignore = "creates and removes an owned local Mail deadline database"]
async fn completion_rechecks_lease_after_blocked_instance_read() {
    fixture(Case::InstanceDeadline).await;
}
#[tokio::test]
#[ignore = "creates and removes an owned local Mail deadline database"]
async fn checkpoint_rechecks_lease_after_blocked_authentication() {
    fixture(Case::AuthDeadline).await;
}
#[tokio::test]
#[ignore = "creates and removes an owned local Mail deadline database"]
async fn guard_rechecks_lease_after_blocked_authentication() {
    fixture(Case::GuardDeadline).await;
}
#[tokio::test]
#[ignore = "creates and removes an owned lifecycle projection-deadline database"]
async fn lifecycle_completion_rolls_back_after_blocked_capacity_projection() {
    fixture(Case::CapacityDeadline).await;
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Domain,
    LockOrder,
    StorageFailure,
    InstanceDeadline,
    AuthDeadline,
    GuardDeadline,
    CapacityDeadline,
}
async fn late_authority(
    owner: &DatabaseConnection,
    service: Arc<IncomingAgent>,
    hosting: Arc<webdock_hosting::HostingService>,
    agent: webdock_hosting::AgentIdentity,
    claim: MailClaim,
    case: Case,
) {
    if matches!(case, Case::InstanceDeadline | Case::CapacityDeadline) {
        service
            .checkpoint(
                hosting.begin_authenticated_agent(&agent).await.unwrap(),
                &agent,
                &claim.spec,
                MailCheckpoint {
                    lease: claim.lease.clone(),
                    runtime_id: "owned-runtime".into(),
                    material_sha256: claim.spec.material_sha256.clone(),
                },
            )
            .await
            .unwrap();
    }
    let blocker = owner.begin().await.unwrap();
    let query = if case == Case::CapacityDeadline {
        "SELECT customer_id FROM webdock_mail.cluster_reservation WHERE customer_id='123' FOR UPDATE"
    } else if case == Case::InstanceDeadline {
        "SELECT customer_id FROM webdock_mail.instance WHERE customer_id='123' FOR UPDATE"
    } else {
        "SELECT id FROM webdock_auth.\"user\" WHERE id='1001' FOR UPDATE"
    };
    sql(&blocker, query, vec![]).await;
    sql(owner,"UPDATE webdock_mail.operation SET lease_until=clock_timestamp()+interval '2 seconds' WHERE id=$1",vec![claim.spec.operation_id.clone().into()]).await;
    let mut task = tokio::spawn(async move {
        let tx = hosting.begin_authenticated_agent(&agent).await.unwrap();
        match case {
            Case::InstanceDeadline | Case::CapacityDeadline => {
                service
                    .complete(
                        tx,
                        &agent,
                        &claim.spec,
                        MailCompletion {
                            lease: claim.lease,
                            outcome: MailOutcome::Succeeded,
                            proof: Some(MailProof {
                                runtime_id: "owned-runtime".into(),
                                namespace: "wd-mail-123".into(),
                                revision: 1,
                                running: true,
                                edition: MailEdition::Community,
                                recovery_disabled: true,
                                submission_certificate_sha256: Some("c".repeat(64)),
                                private_ipv4: None,
                            }),
                        },
                    )
                    .await
            }
            Case::AuthDeadline => {
                service
                    .checkpoint(
                        tx,
                        &agent,
                        &claim.spec,
                        MailCheckpoint {
                            lease: claim.lease,
                            runtime_id: "owned-runtime".into(),
                            material_sha256: claim.spec.material_sha256.clone(),
                        },
                    )
                    .await
            }
            Case::GuardDeadline => service.guard(tx, &agent, &claim.spec, &claim.lease).await,
            Case::Domain | Case::LockOrder | Case::StorageFailure => {
                panic!("deadline case required")
            }
        }
    });
    let blocked=tokio::time::timeout(std::time::Duration::from_secs(1),async{
        loop{
            let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*) AS count FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND pid<>pg_backend_pid()")).await.unwrap().unwrap();
            if row.try_get::<i64>("","count").unwrap()>0{break;}
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await;
    if blocked.is_err() {
        blocker.rollback().await.unwrap();
        task.abort();
        let _ = task.await;
        panic!("callback did not block before its lease expired");
    }
    tokio::time::timeout(std::time::Duration::from_secs(3),async{
        loop{
            let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT lease_until<clock_timestamp() AS expired FROM webdock_mail.operation WHERE customer_id='123' AND state='running'")).await.unwrap().unwrap();
            if row.try_get::<bool>("","expired").unwrap(){break;}
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    blocker.rollback().await.unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), &mut task).await;
    if result.is_err() {
        task.abort();
        let _ = task.await;
        panic!("released callback did not finish promptly");
    }
    let outcome = result.unwrap().unwrap();
    if case == Case::CapacityDeadline {
        assert!(
            matches!(outcome, Err(webdock_mail::incoming::Error::Conflict)),
            "expired capacity projection committed readiness"
        );
    } else if case == Case::GuardDeadline {
        assert!(
            matches!(outcome, Err(webdock_mail::incoming::Error::Conflict)),
            "expired guard was allowed after blocked authentication"
        );
    } else {
        assert!(
            outcome.is_ok(),
            "known expired callback must retain custody without success"
        );
    }
    let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT s.state,s.applied_revision,i.verified_at IS NULL AS unverified,n.checkpoint_live,n.receipt_live FROM webdock_mail.service s JOIN webdock_mail.instance i ON i.customer_id=s.customer_id JOIN webdock_mail.native_operation n ON n.customer_id=s.customer_id WHERE s.customer_id='123'")).await.unwrap().unwrap();
    assert_eq!(
        row.try_get::<i64>("", "applied_revision").unwrap(),
        0,
        "expired callback applied readiness"
    );
    assert!(
        row.try_get::<bool>("", "unverified").unwrap(),
        "expired callback verified an instance"
    );
    match case {
        Case::InstanceDeadline => assert_eq!(
            row.try_get::<Option<bool>>("", "receipt_live").unwrap(),
            Some(false)
        ),
        Case::AuthDeadline => assert_eq!(
            row.try_get::<Option<bool>>("", "checkpoint_live").unwrap(),
            Some(false)
        ),
        Case::GuardDeadline | Case::CapacityDeadline => {
            assert_eq!(row.try_get::<String>("", "state").unwrap(), "provisioning")
        }
        Case::Domain | Case::LockOrder | Case::StorageFailure => panic!("deadline case required"),
    }
}
async fn ordered_claim(
    owner: &DatabaseConnection,
    service: Arc<IncomingAgent>,
    hosting: Arc<webdock_hosting::HostingService>,
    agent: webdock_hosting::AgentIdentity,
) -> MailClaim {
    let admin = owner.begin().await.unwrap();
    sql(
        &admin,
        "SELECT id FROM webdock_admin.customers WHERE id='123' FOR UPDATE",
        vec![],
    )
    .await;
    let mut task = tokio::spawn(async move {
        service
            .claim(
                hosting.begin_authenticated_agent(&agent).await.unwrap(),
                &agent,
            )
            .await
    });
    let waiting=tokio::time::timeout(std::time::Duration::from_secs(2),async{
        loop {
            let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*) AS count FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND pid<>pg_backend_pid()")).await.unwrap().unwrap();
            if row.try_get::<i64>("","count").unwrap()>0 {break;}
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await;
    if waiting.is_err() {
        admin.rollback().await.unwrap();
        task.abort();
        let _ = task.await;
        panic!("worker never reached the held customer fence");
    }
    let service_available=admin.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT customer_id FROM webdock_mail.service WHERE customer_id='123' FOR UPDATE NOWAIT")).await.is_ok();
    let cluster_available = service_available
        && admin
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT id FROM webdock_auth.hosting_cluster WHERE id='200' FOR UPDATE NOWAIT",
            ))
            .await
            .is_ok();
    admin.rollback().await.unwrap();
    if !service_available || !cluster_available {
        task.abort();
        let _ = task.await;
        panic!("worker acquired service or cluster before its customer fence");
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), &mut task).await;
    if result.is_err() {
        task.abort();
        let _ = task.await;
        panic!("serialized worker did not complete promptly");
    }
    result.unwrap().unwrap().unwrap().unwrap()
}
async fn storage_failure(
    owner: &DatabaseConnection,
    service: &IncomingAgent,
    hosting: &webdock_hosting::HostingService,
    agent: &webdock_hosting::AgentIdentity,
    claim: &MailClaim,
) {
    sql(
        owner,
        "REVOKE SELECT ON webdock_auth.\"user\" FROM webdock_auth_bootstrap",
        vec![],
    )
    .await;
    let result = service
        .checkpoint(
            hosting.begin_authenticated_agent(agent).await.unwrap(),
            agent,
            &claim.spec,
            MailCheckpoint {
                lease: claim.lease.clone(),
                runtime_id: "owned-runtime".into(),
                material_sha256: claim.spec.material_sha256.clone(),
            },
        )
        .await;
    sql(
        owner,
        "GRANT SELECT ON webdock_auth.\"user\" TO webdock_auth_bootstrap",
        vec![],
    )
    .await;
    assert!(
        result.is_err(),
        "database failures are not accepted obsolete deliveries"
    );
    let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT (s.state='provisioning' AND o.state='running' AND n.checkpoint_sha256 IS NULL AND n.receipt_sha256 IS NULL) AS unchanged FROM webdock_mail.service s JOIN webdock_mail.operation o ON o.customer_id=s.customer_id JOIN webdock_mail.native_operation n ON n.operation_id=o.id WHERE s.customer_id='123'")).await.unwrap().unwrap();
    assert!(
        row.try_get::<bool>("", "unchanged").unwrap(),
        "transient authority persistence failure changed durable intent"
    );
}
async fn fixture(case: Case) {
    let mut url =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    let admin = Database::connect(url.as_str()).await.unwrap();
    let name = format!("webdock_mail_agent_{}_test", uuid::Uuid::new_v4().simple());
    admin
        .execute_unprepared(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    url.set_path(&format!("/{name}"));
    let owner = Database::connect(url.as_str()).await.unwrap();
    let result=std::panic::AssertUnwindSafe(async{
        webdock_persistence::migrations::bootstrap(&owner,&name,KEY).await.unwrap();
        sql(&owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES('1001','Operator','operator@example.invalid',true,'operator',true,false)",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES('operator-session','operator-token','1001',now()+interval '1 hour',now())",vec![]).await;
        let tx=owner.begin().await.unwrap();
        sql(&tx,"SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"123\",\"organization_id\":\"124\"}',true)",vec![]).await;
        sql(&tx,"INSERT INTO webdock_admin.customers(id,name) VALUES('123','Incoming agent fixture')",vec![]).await;tx.commit().await.unwrap();
        sql(&owner,"INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":true}')",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('mail','builtin','enabled','enabled','test-only')",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,verified,capacity) VALUES('200','Owned Mail fixture','managed','eu','fixture',true,$1)",vec![json!({"apps":2,"cpuMillicores":1000,"memoryBytes":2147483648u64,"volumeBytes":21474836480u64,"ephemeralBytes":536870912,"replicasPerApp":1,"concurrentDeployments":2}).into()]).await;
        let credential="a".repeat(43);
        sql(&owner,"INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_seen,observation) VALUES('200',$1,1,now(),$2)",vec![hex::encode(Sha256::digest(credential.as_bytes())).into(),json!({"version":"v1.36.5+k3s1","runtimeKind":"kubernetes","capabilities":{"nativeMail":{"version":1,"image":IMAGE},"storageVersion":1}}).into()]).await;
        let auth=role("AUTH_DATABASE_URL",&name,&url).await;let business=role("DATABASE_URL",&name,&url).await;let worker=role("WEBDOCK_PLUGIN_WORKER_DATABASE_URL",&name,&url).await;
        let cookies=CookiePolicy::new(false,"incoming-agent-cookie-key-atleast32").unwrap();
        let cookie=format!("{}={}",cookies.name(),cookies.sign("operator-token").unwrap());
        let identity=Arc::new(IdentityService::new(auth.clone(),cookies));
        let principal=identity.authenticate_cookie(&cookie).await.unwrap().unwrap();
        let ids=Arc::new(webdock_persistence::IdAllocator::new(role("DATABASE_URL",&name,&url).await,webdock_ids::Identity::platform(0,2).unwrap()));
        let hosting=Arc::new(webdock_hosting::HostingService::new(business.clone(),auth.clone(),worker.clone(),KEY.into(),"incoming-platform-encryption-key-atleast32".into(),ids.clone()).await.unwrap());
        let incoming=Arc::new(NativeIncoming::new(MailDependencies{auth:auth.clone(),business:business.clone(),worker:worker.clone(),identity:identity.clone(),browser:None,ids},IncomingConfig{enabled:true,mailboxes_enabled:true,sending_enabled:true,managed_cluster:"200".into(),rls_secret:KEY.into()}).await.unwrap());
        let service=Arc::new(IncomingAgent::new(incoming.clone(),MailAgentConfig{image:IMAGE.into(),hostname_suffix:"example.test".into(),encryption_key:STANDARD.encode([37u8;32])}).unwrap());
        let agent=hosting.authenticate_agent("200",&credential).await.unwrap();
        assert!(hosting.claim_runtime(&agent).await.unwrap().is_none());
        assert!(hosting.claim_read(&agent).await.unwrap().is_none());
        assert!(service.claim(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().is_none());
        incoming.manage_service(&principal,&StoredAuthority::LocalSession {},"123",ServiceCommand::Activate{revision:"0".into()}).await.unwrap();
        let claim=if case==Case::LockOrder {ordered_claim(&owner,service.clone(),hosting.clone(),agent.clone()).await}
            else {service.claim(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap()};
        if case==Case::LockOrder {assert_eq!(incoming.service(&principal,&StoredAuthority::LocalSession {},"123").await.unwrap().service.state,ServiceState::Provisioning);return;}
        if case==Case::StorageFailure {storage_failure(&owner,&service,&hosting,&agent,&claim).await;return;}
        if matches!(case,Case::InstanceDeadline|Case::AuthDeadline|Case::GuardDeadline|Case::CapacityDeadline){late_authority(&owner,service.clone(),hosting.clone(),agent.clone(),claim,case).await;return;}
        claim.validate(chrono::Utc::now()).unwrap();
        let MailMaterial::Credentials{bootstrap_password,username,password}=&claim.material else {panic!("credentials expected")};
        assert!(bootstrap_password.is_some());
        let replay=service.claim(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap();
        assert_eq!(replay.spec.fingerprint().unwrap(),claim.spec.fingerprint().unwrap());assert!(replay.lease.lease_token==claim.lease.lease_token,"replayed lease changed");assert_eq!(replay.lease_until,claim.lease_until);
        sql(&owner,"UPDATE webdock_mail.operation SET lease_token='22222222-2222-4222-8222-222222222222' WHERE id=$1",vec![claim.spec.operation_id.clone().into()]).await;
        assert!(service.claim(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.is_err(),"altered persisted lease must not redeliver material");
        sql(&owner,"UPDATE webdock_mail.operation SET lease_token=$2 WHERE id=$1",vec![claim.spec.operation_id.clone().into(),claim.lease.lease_token.clone().into()]).await;
        assert!(owner.execute_unprepared("UPDATE webdock_mail.native_operation SET request_sha256=repeat('b',64) WHERE request_sha256 IS NOT NULL").await.is_err(),"issued provenance is immutable");
        service.guard(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&claim.spec,&claim.lease).await.unwrap();
        let checkpoint=MailCheckpoint{lease:claim.lease.clone(),runtime_id:"owned-runtime-1".into(),material_sha256:claim.spec.material_sha256.clone()};
        service.checkpoint(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&claim.spec,checkpoint.clone()).await.unwrap();
        service.checkpoint(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&claim.spec,checkpoint.clone()).await.unwrap();
        let mut forged=checkpoint.clone();forged.runtime_id="changed-runtime".into();
        assert!(service.checkpoint(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&claim.spec,forged).await.is_err());
        let proof=MailProof{runtime_id:"owned-runtime-2".into(),namespace:"wd-mail-123".into(),revision:1,running:true,edition:MailEdition::Community,recovery_disabled:true,submission_certificate_sha256:Some("b".repeat(64)),private_ipv4:None};
        let receipt=MailCompletion{lease:claim.lease.clone(),outcome:MailOutcome::Succeeded,proof:Some(proof)};
        let before=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT encrypted_credentials FROM webdock_mail.instance WHERE customer_id='123'")).await.unwrap().unwrap();
        let before_cipher:String=before.try_get("","encrypted_credentials").unwrap();
        let mut missing_pin=receipt.clone();missing_pin.proof.as_mut().unwrap().submission_certificate_sha256=None;
        assert!(service.complete(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&claim.spec,missing_pin).await.is_err(),"missing running TLS pin must not make Mail ready");
        let unchanged=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT i.encrypted_credentials,(s.state='provisioning' AND s.applied_revision=0 AND i.verified_at IS NULL AND n.receipt_sha256 IS NULL) AS pending FROM webdock_mail.instance i JOIN webdock_mail.service s ON s.customer_id=i.customer_id JOIN webdock_mail.native_operation n ON n.customer_id=i.customer_id WHERE i.customer_id='123'")).await.unwrap().unwrap();
        assert!(unchanged.try_get::<bool>("","pending").unwrap());
        assert!(unchanged.try_get::<String>("","encrypted_credentials").unwrap()==before_cipher,"rejected completion must retain bootstrap ciphertext unchanged");
        service.complete(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&claim.spec,receipt.clone()).await.unwrap();
        service.complete(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&claim.spec,receipt.clone()).await.unwrap();
        assert_eq!(incoming.service(&principal,&StoredAuthority::LocalSession {},"123").await.unwrap().service.state,ServiceState::Ready);
        let mut changed=receipt;changed.proof.as_mut().unwrap().runtime_id="forged".into();
        assert!(service.complete(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&claim.spec,changed).await.is_err());
        incoming.manage_service(&principal,&StoredAuthority::LocalSession {},"123",ServiceCommand::Suspend{revision:"1".into()}).await.unwrap();
        let suspend=service.claim(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap();
        assert_eq!(suspend.spec.action,MailAction::Suspend);
        let MailMaterial::Credentials{bootstrap_password:resumed_bootstrap,username:resumed_username,password:resumed_password}=&suspend.material else {panic!("retained credentials expected")};
        assert!(resumed_bootstrap.is_none());assert_eq!(resumed_username,username);assert!(resumed_password==password,"retained password changed");
        sql(&owner,"UPDATE webdock_mail.operation SET lease_until=now()-interval '1 second' WHERE id=$1",vec![suspend.spec.operation_id.clone().into()]).await;
        assert!(service.guard(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&suspend.spec,&suspend.lease).await.is_err());
        let stale=MailCompletion{lease:suspend.lease.clone(),outcome:MailOutcome::Succeeded,proof:Some(MailProof{runtime_id:"owned-runtime-2".into(),namespace:"wd-mail-123".into(),revision:2,running:false,edition:MailEdition::Community,recovery_disabled:true,submission_certificate_sha256:None,private_ipv4:None})};
        service.complete(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&suspend.spec,stale.clone()).await.unwrap();
        service.complete(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&suspend.spec,stale).await.unwrap();
        assert_eq!(incoming.service(&principal,&StoredAuthority::LocalSession {},"123").await.unwrap().service.state,ServiceState::NeedsReview);
        incoming.manage_service(&principal,&StoredAuthority::LocalSession {},"123",ServiceCommand::Reconcile{revision:"2".into()}).await.unwrap();
        let older=service.claim(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap();
        incoming.manage_service(&principal,&StoredAuthority::LocalSession {},"123",ServiceCommand::Activate{revision:"3".into()}).await.unwrap();
        let failure=MailCompletion{lease:older.lease.clone(),outcome:MailOutcome::NeedsReview,proof:None};
        service.complete(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&older.spec,failure).await.unwrap();
        let blocked=incoming.service(&principal,&StoredAuthority::LocalSession {},"123").await.unwrap();
        assert_eq!(blocked.service.revision,"4");assert_eq!(blocked.service.state,ServiceState::Pending);
        assert!(blocked.can_reconcile,"an older uncertain operation must be explicitly reviewable without overwriting newer desired state");
        incoming.manage_service(&principal,&StoredAuthority::LocalSession {},"123",ServiceCommand::Reconcile{revision:"4".into()}).await.unwrap();
        let resumed=service.claim(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap();
        assert_eq!(resumed.spec.revision,5);assert_eq!(resumed.spec.action,MailAction::Ensure);
        service.complete(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&resumed.spec,MailCompletion{lease:resumed.lease,outcome:MailOutcome::NeedsReview,proof:None}).await.unwrap();
        let allocation=owner.begin().await.unwrap();
        sql(&allocation,"SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"125\",\"organization_id\":\"126\"}',true)",vec![]).await;
        sql(&allocation,"INSERT INTO webdock_admin.customers(id,name) VALUES('125','Expired first setup')",vec![]).await;allocation.commit().await.unwrap();
        incoming.manage_service(&principal,&StoredAuthority::LocalSession {},"125",ServiceCommand::Activate{revision:"0".into()}).await.unwrap();
        let first=service.claim(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap();assert_eq!(first.spec.customer_id,"125");
        sql(&owner,"UPDATE webdock_mail.operation SET lease_until=now()-interval '1 second' WHERE id=$1",vec![first.spec.operation_id.clone().into()]).await;
        assert!(service.claim(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().is_none());
        let blocked=incoming.service(&principal,&StoredAuthority::LocalSession {},"125").await.unwrap();
        assert_eq!(blocked.service.state,ServiceState::NeedsReview);assert!(!blocked.can_reconcile);
        assert!(incoming.manage_service(&principal,&StoredAuthority::LocalSession {},"125",ServiceCommand::Reconcile{revision:"1".into()}).await.is_err());
        let unverified=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT verified_at IS NULL AS unverified FROM webdock_mail.instance WHERE customer_id='125'")).await.unwrap().unwrap();
        assert!(unverified.try_get::<bool>("","unverified").unwrap(),"lease expiry must never manufacture readiness");
        drop(service);drop(incoming);drop(hosting);drop(identity);auth.close().await.unwrap();business.close().await.unwrap();worker.close().await.unwrap();
    }).catch_unwind().await;
    owner.close().await.unwrap();
    admin
        .execute_unprepared(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    admin.close().await.unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
