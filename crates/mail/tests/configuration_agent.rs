//! Restricted database authority/receipt checks. Runtime proofs here are controlled inputs;
//! the separate real-engine acceptance must establish external lifecycle behavior.
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::FutureExt;
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TransactionTrait,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use webdock_agent_protocol::mail::*;
use webdock_agent_protocol::mail_configuration::*;
use webdock_browser_session::StoredAuthority;
use webdock_identity::{CookiePolicy, IdentityService};
use webdock_mail::{
    MailDependencies,
    incoming::{
        IncomingConfig, MailboxCommand, NativeIncoming, ServiceCommand,
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
#[ignore = "creates and drops an owned native Mail configuration database"]
async fn configuration_admission_preserves_original_authority_and_execution_custody() {
    fixture(Case::Full).await;
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Full,
    Historical,
    Revoked,
    Deadline,
    Storage,
    CheckpointDeadline,
    CompletionDeadline,
    PendingRegrant,
    MemberLock,
    MemberSwap,
    MaterializedRegrant,
}
#[tokio::test]
#[ignore = "owned local configuration history database"]
async fn historical_configuration_requires_fresh_explicit_admission() {
    fixture(Case::Historical).await;
}
#[tokio::test]
#[ignore = "owned local configuration authority database"]
async fn original_session_revocation_prevents_configuration_issue() {
    fixture(Case::Revoked).await;
}
#[tokio::test]
#[ignore = "owned local configuration deadline database"]
async fn configuration_guard_expires_after_blocked_original_authentication() {
    fixture(Case::Deadline).await;
}
#[tokio::test]
#[ignore = "owned local configuration auth-storage database"]
async fn configuration_auth_storage_failure_keeps_work_unsettled() {
    fixture(Case::Storage).await;
}
#[tokio::test]
#[ignore = "owned local configuration projection-deadline database"]
async fn applied_checkpoint_rolls_back_if_provider_reference_write_outlives_lease() {
    fixture(Case::CheckpointDeadline).await;
}
#[tokio::test]
#[ignore = "owned local configuration projection-deadline database"]
async fn configuration_completion_rolls_back_if_projection_write_outlives_lease() {
    fixture(Case::CompletionDeadline).await;
}
#[tokio::test]
#[ignore = "owned local configuration regrant database"]
async fn unissued_human_restriction_cannot_strand_a_newer_regrant() {
    fixture(Case::PendingRegrant).await;
}
#[tokio::test]
#[ignore = "owned local nonoperator membership lock database"]
async fn nonoperator_member_fence_precedes_configuration_row_locks() {
    fixture(Case::MemberLock).await;
}
#[tokio::test]
#[ignore = "owned local original membership database"]
async fn replacement_membership_cannot_revive_original_configuration_authority() {
    fixture(Case::MemberSwap).await;
}
#[tokio::test]
#[ignore = "owned local materialized restriction regrant database"]
async fn materialized_membership_restriction_cannot_strand_a_newer_regrant() {
    fixture(Case::MaterializedRegrant).await;
}
async fn fixture(case: Case) {
    let mut url =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    let admin = Database::connect(url.as_str()).await.unwrap();
    let name = format!("webdock_mail_config_{}_test", uuid::Uuid::new_v4().simple());
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
        sql(&owner,"INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_seen,observation) VALUES('200',$1,1,now(),$2)",vec![hex::encode(Sha256::digest(credential.as_bytes())).into(),json!({"version":"v1.36.5+k3s1","runtimeKind":"kubernetes","capabilities":{"nativeMail":{"version":1,"image":IMAGE,"configurationVersion":1},"storageVersion":1}}).into()]).await;
        let auth=role("AUTH_DATABASE_URL",&name,&url).await;let business=role("DATABASE_URL",&name,&url).await;let worker=role("WEBDOCK_PLUGIN_WORKER_DATABASE_URL",&name,&url).await;
        let cookies=CookiePolicy::new(false,"incoming-agent-cookie-key-atleast32").unwrap();
        let cookie=format!("{}={}",cookies.name(),cookies.sign("operator-token").unwrap());
        let identity=Arc::new(IdentityService::new(auth.clone(),cookies));
        let principal=identity.authenticate_cookie(&cookie).await.unwrap().unwrap();
        let ids=Arc::new(webdock_persistence::IdAllocator::new(role("DATABASE_URL",&name,&url).await,webdock_ids::Identity::platform(0,2).unwrap()));
        let hosting=Arc::new(webdock_hosting::HostingService::new(business.clone(),auth.clone(),worker.clone(),KEY.into(),"incoming-platform-encryption-key-atleast32".into(),ids.clone()).await.unwrap());
        let incoming=Arc::new(NativeIncoming::new(MailDependencies{auth:auth.clone(),business:business.clone(),worker:worker.clone(),identity:identity.clone(),browser:None,ids},IncomingConfig{enabled:true,mailboxes_enabled:true,sending_enabled:true,managed_cluster:"200".into(),rls_secret:KEY.into()}).await.unwrap());
        let service=Arc::new(IncomingAgent::new(incoming.clone(),MailAgentConfig{image:IMAGE.into(),hostname_suffix:"example.test".into(),encryption_key:STANDARD.encode([37u8;32])}).unwrap().with_configuration_enabled(true));
        let agent=hosting.authenticate_agent("200",&credential).await.unwrap();
        incoming.manage_service(&principal,&StoredAuthority::LocalSession {},"123",ServiceCommand::Activate{revision:"0".into()}).await.unwrap();
        let lifecycle=service.claim(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap();
        service.checkpoint(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&lifecycle.spec,MailCheckpoint{lease:lifecycle.lease.clone(),runtime_id:"normal-runtime".into(),material_sha256:lifecycle.spec.material_sha256.clone()}).await.unwrap();
        service.complete(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&lifecycle.spec,MailCompletion{lease:lifecycle.lease,outcome:MailOutcome::Succeeded,proof:Some(MailProof{runtime_id:"normal-runtime".into(),namespace:"wd-mail-123".into(),revision:1,running:true,edition:MailEdition::Community,recovery_disabled:true,submission_certificate_sha256:Some("b".repeat(64)),private_ipv4:None})}).await.unwrap();
        sql(&owner,"INSERT INTO webdock_auth.mail_sender_domain(id,customer_id,domain,token,status) VALUES('301','123','example.test','fixture','ownership_verified')",vec![]).await;
        if matches!(case,Case::MemberLock|Case::MemberSwap){
            nonoperator_case(&owner,&incoming,service.clone(),hosting.clone(),agent.clone(),&identity,case).await;return;
        }
        incoming.manage_mailboxes(&principal,&StoredAuthority::LocalSession {},"123",MailboxCommand::DomainAttach{sender_domain_id:"301".into()}).await.unwrap();
        let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT subject,session_id,native_authority FROM webdock_mail.native_configuration_operation")).await.unwrap().unwrap();
        assert_eq!(row.try_get::<String>("","subject").unwrap(),"1001");
        assert_eq!(row.try_get::<String>("","session_id").unwrap(),"operator-session");
        assert_eq!(row.try_get::<serde_json::Value>("","native_authority").unwrap(),json!({"kind":"local-session"}));
        if case==Case::Historical{
            sql(&owner,"DELETE FROM webdock_mail.native_configuration_operation",vec![]).await;
            assert!(service.claim_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().is_none());
            let directory=incoming.directory(&principal,&StoredAuthority::LocalSession {},"123").await.unwrap();assert!(directory.can_retry_configuration);
            incoming.manage_mailboxes(&principal,&StoredAuthority::LocalSession {},"123",MailboxCommand::ConfigurationRetry{revision:directory.configuration.unwrap().revision}).await.unwrap();
            let admitted=service.claim_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap();assert_eq!(admitted.spec.revision,"2");return;
        }
        if case==Case::Revoked{
            sql(&owner,"UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 second' WHERE id='operator-session'",vec![]).await;
            assert!(service.claim_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().is_none());
            let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT o.state,n.request_sha256 FROM webdock_mail.configuration_operation o JOIN webdock_mail.native_configuration_operation n ON n.operation_id=o.id")).await.unwrap().unwrap();assert_eq!(row.try_get::<String>("","state").unwrap(),"needs_review");assert!(row.try_get::<Option<String>>("","request_sha256").unwrap().is_none());return;
        }
        let claim=service.claim_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap();
        claim.validate(chrono::Utc::now()).unwrap();
        if matches!(case,Case::CheckpointDeadline|Case::CompletionDeadline){late_callback(&owner,service.clone(),hosting.clone(),agent.clone(),claim,case==Case::CompletionDeadline).await;return;}
        if case==Case::Deadline{late_guard(&owner,service.clone(),hosting.clone(),agent.clone(),claim).await;return;}
        if case==Case::Storage{
            sql(&owner,"REVOKE SELECT ON webdock_auth.\"user\" FROM webdock_auth_bootstrap",vec![]).await;
            let outcome=service.guard_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&claim.spec,&claim.lease).await;
            sql(&owner,"GRANT SELECT ON webdock_auth.\"user\" TO webdock_auth_bootstrap",vec![]).await;
            assert!(matches!(outcome,Err(webdock_mail::incoming::Error::Unavailable)));
            let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT o.state,n.receipt_sha256 FROM webdock_mail.configuration_operation o JOIN webdock_mail.native_configuration_operation n ON n.operation_id=o.id")).await.unwrap().unwrap();assert_eq!(row.try_get::<String>("","state").unwrap(),"running");assert!(row.try_get::<Option<String>>("","receipt_sha256").unwrap().is_none());return;
        }

        let repeated=service.claim_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap();
        assert_eq!(claim.spec.fingerprint().unwrap(),repeated.spec.fingerprint().unwrap());assert_eq!(claim.lease_until,repeated.lease_until);
        assert!(owner.execute_unprepared("UPDATE webdock_mail.native_configuration_operation SET session_id='changed'").await.is_err());
        let receipt=finish(&service,&hosting,&agent,&claim).await;
        service.complete_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&claim.spec,receipt.clone()).await.unwrap();
        let mut changed=receipt;changed.proof.as_mut().unwrap().runtime_id="replacement".into();
        assert!(service.complete_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&claim.spec,changed).await.is_err());
        let directory=incoming.directory(&principal,&StoredAuthority::LocalSession {},"123").await.unwrap();
        let domain=directory.domains[0].id.clone();assert_eq!(directory.configuration.unwrap().applied_revision,"1");
        let mut mailboxes=vec![];
        for local in ["first","second"]{
            let result=incoming.manage_mailboxes(&principal,&StoredAuthority::LocalSession {},"123",MailboxCommand::MailboxCreate{domain_id:domain.clone(),local_part:local.into(),name:local.into(),quota_bytes:1048576}).await.unwrap();
            let mailbox=result.id.unwrap();
            incoming.manage_mailboxes(&principal,&StoredAuthority::LocalSession {},"123",MailboxCommand::Grant{mailbox_id:mailbox.clone(),subject:"1001".into()}).await.unwrap();mailboxes.push(mailbox);
        }
        let full=service.claim_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap();
        assert_eq!(full.spec.snapshot.identities.len(),1);let credential_hash=full.material.fingerprint().unwrap();
        finish(&service,&hosting,&agent,&full).await;
        if case==Case::PendingRegrant {
            incoming.manage_mailboxes(&principal,&StoredAuthority::LocalSession {},"123",MailboxCommand::Revoke{mailbox_id:mailboxes[0].clone(),subject:"1001".into()}).await.unwrap();
            incoming.manage_mailboxes(&principal,&StoredAuthority::LocalSession {},"123",MailboxCommand::Grant{mailbox_id:mailboxes[0].clone(),subject:"1001".into()}).await.unwrap();
            let mut progressed=false;
            for _ in 0..3 {
                if let Ok(Some(next))=service.claim_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await {
                    assert_eq!(next.spec.mode,ConfigurationMode::Apply,"obsolete unissued restriction must not reach the runtime");
                    progressed=true;
                    break;
                }
            }
            let states=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT jsonb_agg(jsonb_build_object('mode',n.mode,'state',o.state,'issued',n.request_sha256 IS NOT NULL) ORDER BY o.revision) AS states FROM webdock_mail.configuration_operation o JOIN webdock_mail.native_configuration_operation n ON n.operation_id=o.id WHERE o.customer_id='123' AND o.state IN ('pending','running','needs_review')")).await.unwrap().unwrap().try_get::<serde_json::Value>("","states").unwrap();
            assert!(progressed,"regrant never becomes dispatchable because stale restriction remains selected: {states}");
            return;
        }
        sql(&owner,"UPDATE webdock_auth.plugin_installation SET state='draining' WHERE id='mail'",vec![]).await;
        incoming.manage_mailboxes(&principal,&StoredAuthority::LocalSession {},"123",MailboxCommand::Revoke{mailbox_id:mailboxes[0].clone(),subject:"1001".into()}).await.unwrap();
        let restriction=service.claim_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap();
        assert_eq!(restriction.spec.mode,ConfigurationMode::Restrict);assert_eq!(restriction.spec.snapshot.mailboxes.len(),1);
        assert_eq!(restriction.spec.snapshot.identities[0].mailbox_ids,vec![mailboxes[1].clone()]);assert!(restriction.material.identities.is_empty());
        service.checkpoint_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&restriction.spec,ConfigurationCheckpoint{lease:restriction.lease.clone(),runtime_id:"normal-runtime".into(),phase:ConfigurationCheckpointPhase::Bound{}}).await.unwrap();
        let forged=ConfigurationCheckpoint{lease:restriction.lease.clone(),runtime_id:"normal-runtime".into(),phase:ConfigurationCheckpointPhase::Handoff{resource:ConfigurationResource{kind:ConfigurationResourceKind::Mailbox,id:mailboxes[1].clone()},request_sha256:"a".repeat(64)}};
        assert!(service.checkpoint_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent,&restriction.spec,forged).await.is_err());
        finish(&service,&hosting,&agent,&restriction).await;
        let directory=incoming.directory(&principal,&StoredAuthority::LocalSession {},"123").await.unwrap();
        let configuration=directory.configuration.unwrap();assert_eq!(configuration.state,Some(webdock_mail::incoming::ConfigurationState::Succeeded));assert_ne!(configuration.revision,configuration.applied_revision);assert!(!directory.can_retry_configuration);
        sql(&owner,"UPDATE webdock_auth.plugin_installation SET state='enabled' WHERE id='mail'",vec![]).await;
        assert!(incoming.directory(&principal,&StoredAuthority::LocalSession {},"123").await.unwrap().can_retry_configuration);
        incoming.manage_mailboxes(&principal,&StoredAuthority::LocalSession {},"123",MailboxCommand::ConfigurationRetry{revision:configuration.revision}).await.unwrap();
        let restored=service.claim_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap();
        assert_eq!(restored.spec.mode,ConfigurationMode::Apply);assert_eq!(restored.material.fingerprint().unwrap(),credential_hash);
        finish(&service,&hosting,&agent,&restored).await;
        let submission=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT submission_state FROM webdock_mail.instance WHERE customer_id='123'")).await.unwrap().unwrap();assert!(submission.try_get::<Option<String>>("","submission_state").unwrap().is_none());
        sql(&owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"mustChangePassword\") VALUES('1002','Member','member@example.test',true,'user',false)",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('native-config-member','124','1002','member',now())",vec![]).await;
        incoming.manage_mailboxes(&principal,&StoredAuthority::LocalSession {},"123",MailboxCommand::Grant{mailbox_id:mailboxes[0].clone(),subject:"1002".into()}).await.unwrap();
        incoming.manage_mailboxes(&principal,&StoredAuthority::LocalSession {},"123",MailboxCommand::Grant{mailbox_id:mailboxes[1].clone(),subject:"1002".into()}).await.unwrap();
        let granted=service.claim_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap();finish(&service,&hosting,&agent,&granted).await;
        sql(&owner,"UPDATE webdock_auth.plugin_installation SET state='draining' WHERE id='mail'",vec![]).await;
        sql(&owner,"DELETE FROM webdock_auth.member WHERE id='native-config-member'",vec![]).await;
        sql(&owner,"UPDATE webdock_auth.plugin_installation SET state='enabled' WHERE id='mail'",vec![]).await;
        if case==Case::MaterializedRegrant{
            let event=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT configuration_revision FROM webdock_mail.native_configuration_restriction WHERE customer_id='123' ORDER BY configuration_revision LIMIT 1")).await.unwrap().unwrap();let revision=event.try_get::<i64>("","configuration_revision").unwrap();
            let allocated=webdock_persistence::IdAllocator::new(business.clone(),webdock_ids::Identity::platform(0,2).unwrap()).allocate().await.unwrap().to_string();
            let tx=hosting.begin_authenticated_agent(&agent).await.unwrap();let result=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT webdock_mail.admit_configuration_restriction('123',$1,$2) AS admitted",[revision.into(),allocated.clone().into()])).await.unwrap().unwrap();assert!(result.try_get::<bool>("","admitted").unwrap());tx.commit().await.unwrap();
            sql(&owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('regranted-native-member','124','1002','member',now())",vec![]).await;
            for mailbox in &mailboxes{incoming.manage_mailboxes(&principal,&StoredAuthority::LocalSession {},"123",MailboxCommand::Grant{mailbox_id:mailbox.clone(),subject:"1002".into()}).await.unwrap();}
            let mut progressed=false;for _ in 0..3{if let Some(next)=service.claim_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap(){assert_eq!(next.spec.mode,ConfigurationMode::Apply);finish(&service,&hosting,&agent,&next).await;progressed=true;break;}}
            assert!(progressed,"materialized old restriction prevented a fresh full grant");
            let old=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT o.state,n.request_sha256 FROM webdock_mail.configuration_operation o JOIN webdock_mail.native_configuration_operation n ON n.operation_id=o.id WHERE o.id=$1",[allocated.into()])).await.unwrap().unwrap();assert_eq!(old.try_get::<String>("","state").unwrap(),"superseded");assert!(old.try_get::<Option<String>>("","request_sha256").unwrap().is_none());
            let remaining=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*) AS count FROM webdock_mail.native_configuration_restriction WHERE operation_id IS NULL AND NOT obsolete")).await.unwrap().unwrap();assert_eq!(remaining.try_get::<i64>("","count").unwrap(),0);return;
        }
        incoming.manage_mailboxes(&principal,&StoredAuthority::LocalSession {},"123",MailboxCommand::MailboxCreate{domain_id:domain.clone(),local_part:"concurrent".into(),name:"Concurrent desired mailbox".into(),quota_bytes:1048576}).await.unwrap();
        assert!(worker.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT webdock_mail.admit_configuration_restriction('123',1,'999')")).await.is_err());
        let claim_a=async{service.claim_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap()};
        let claim_b=async{service.claim_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap()};
        let (restricted,replay)=tokio::join!(claim_a,claim_b);assert_eq!(restricted.spec.operation_id,replay.spec.operation_id);assert_eq!(restricted.lease.lease_token,replay.lease.lease_token);
        let provenance=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT subject,session_id,native_authority FROM webdock_mail.native_configuration_operation WHERE provenance_kind='restriction'")).await.unwrap().unwrap();
        assert!(provenance.try_get::<Option<String>>("","subject").unwrap().is_none());assert!(provenance.try_get::<Option<String>>("","session_id").unwrap().is_none());assert!(provenance.try_get::<Option<serde_json::Value>>("","native_authority").unwrap().is_none());
        assert_eq!(restricted.spec.mode,ConfigurationMode::Restrict);assert!(restricted.spec.snapshot.identities.iter().find(|i|i.subject=="1002").unwrap().mailbox_ids.is_empty());finish(&service,&hosting,&agent,&restricted).await;
        let next=service.claim_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap();assert_eq!(next.spec.mode,ConfigurationMode::Restrict);finish(&service,&hosting,&agent,&next).await;
        let full=service.claim_configuration(hosting.begin_authenticated_agent(&agent).await.unwrap(),&agent).await.unwrap().unwrap();assert_eq!(full.spec.mode,ConfigurationMode::Apply);finish(&service,&hosting,&agent,&full).await;
        let outstanding=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*) AS count FROM webdock_mail.native_configuration_restriction WHERE operation_id IS NULL")).await.unwrap().unwrap();assert_eq!(outstanding.try_get::<i64>("","count").unwrap(),0);

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

async fn prepare(
    service: &IncomingAgent,
    hosting: &webdock_hosting::HostingService,
    agent: &webdock_hosting::AgentIdentity,
    claim: &ConfigurationClaim,
) -> ConfigurationCompletion {
    let mut proof = ConfigurationProof {
        runtime_id: "normal-runtime".into(),
        domains: BTreeMap::new(),
        mailboxes: BTreeMap::new(),
        identities: BTreeMap::new(),
        dkim: BTreeMap::new(),
    };
    let bound = ConfigurationCheckpoint {
        lease: claim.lease.clone(),
        runtime_id: proof.runtime_id.clone(),
        phase: ConfigurationCheckpointPhase::Bound {},
    };
    service
        .checkpoint_configuration(
            hosting.begin_authenticated_agent(agent).await.unwrap(),
            agent,
            &claim.spec,
            bound,
        )
        .await
        .unwrap();
    let mut resources = vec![];
    for domain in &claim.spec.snapshot.domains {
        let provider = domain
            .provider_id
            .clone()
            .unwrap_or_else(|| format!("domain-{}", domain.id));
        proof.domains.insert(domain.id.clone(), provider.clone());
        proof.dkim.insert(
            domain.id.clone(),
            vec![ConfigurationDkim {
                id: format!("dkim-{}", domain.id),
                selector: "webdock-rsa-20261020".into(),
                public_key: include_str!("configuration_public_key.txt").trim().into(),
            }],
        );
        resources.push((
            ConfigurationResource {
                kind: ConfigurationResourceKind::Domain,
                id: domain.id.clone(),
            },
            provider,
        ));
    }
    for mailbox in &claim.spec.snapshot.mailboxes {
        let provider = mailbox
            .provider_id
            .clone()
            .unwrap_or_else(|| format!("mailbox-{}", mailbox.id));
        proof.mailboxes.insert(mailbox.id.clone(), provider.clone());
        if claim.spec.mode == ConfigurationMode::Apply {
            resources.push((
                ConfigurationResource {
                    kind: ConfigurationResourceKind::Mailbox,
                    id: mailbox.id.clone(),
                },
                provider,
            ));
        }
    }
    for identity in &claim.spec.snapshot.identities {
        let provider = identity.provider_id.clone().or_else(|| {
            identity
                .enabled
                .then(|| format!("identity-{}", identity.id))
        });
        proof
            .identities
            .insert(identity.id.clone(), provider.clone());
        if let Some(provider) = provider {
            resources.push((
                ConfigurationResource {
                    kind: ConfigurationResourceKind::Identity,
                    id: identity.id.clone(),
                },
                provider,
            ));
        }
    }
    for (resource, provider_id) in resources {
        let request_sha256 = webdock_agent_protocol::mail::fingerprint(&resource).unwrap();
        for phase in [
            ConfigurationCheckpointPhase::Handoff {
                resource: resource.clone(),
                request_sha256: request_sha256.clone(),
            },
            ConfigurationCheckpointPhase::Applied {
                resource,
                request_sha256,
                provider_id,
            },
        ] {
            let callback = ConfigurationCheckpoint {
                lease: claim.lease.clone(),
                runtime_id: proof.runtime_id.clone(),
                phase,
            };
            service
                .checkpoint_configuration(
                    hosting.begin_authenticated_agent(agent).await.unwrap(),
                    agent,
                    &claim.spec,
                    callback.clone(),
                )
                .await
                .unwrap();
            service
                .checkpoint_configuration(
                    hosting.begin_authenticated_agent(agent).await.unwrap(),
                    agent,
                    &claim.spec,
                    callback,
                )
                .await
                .unwrap();
        }
    }
    ConfigurationCompletion {
        lease: claim.lease.clone(),
        outcome: MailOutcome::Succeeded,
        proof: Some(proof),
    }
}

async fn late_guard(
    owner: &DatabaseConnection,
    service: Arc<IncomingAgent>,
    hosting: Arc<webdock_hosting::HostingService>,
    agent: webdock_hosting::AgentIdentity,
    claim: ConfigurationClaim,
) {
    let block = owner.begin().await.unwrap();
    sql(
        &block,
        "SELECT id FROM webdock_auth.\"user\" WHERE id='1001' FOR UPDATE",
        vec![],
    )
    .await;
    sql(owner,"UPDATE webdock_mail.configuration_operation SET lease_until=clock_timestamp()+interval '2 seconds' WHERE id=$1",vec![claim.spec.operation_id.clone().into()]).await;
    let mut task = tokio::spawn(async move {
        service
            .guard_configuration(
                hosting.begin_authenticated_agent(&agent).await.unwrap(),
                &agent,
                &claim.spec,
                &claim.lease,
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(1),async{loop{let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND pid<>pg_backend_pid()) AS blocked")).await.unwrap().unwrap();if row.try_get::<bool>("","blocked").unwrap(){break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3),async{loop{let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT lease_until<clock_timestamp() AS expired FROM webdock_mail.configuration_operation WHERE state='running'")).await.unwrap().unwrap();if row.try_get::<bool>("","expired").unwrap(){break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}).await.unwrap();
    block.rollback().await.unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), &mut task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        result,
        Err(webdock_mail::incoming::Error::Conflict)
    ));
}

async fn finish(
    service: &IncomingAgent,
    hosting: &webdock_hosting::HostingService,
    agent: &webdock_hosting::AgentIdentity,
    claim: &ConfigurationClaim,
) -> ConfigurationCompletion {
    let receipt = prepare(service, hosting, agent, claim).await;
    service
        .complete_configuration(
            hosting.begin_authenticated_agent(agent).await.unwrap(),
            agent,
            &claim.spec,
            receipt.clone(),
        )
        .await
        .unwrap();
    receipt
}
async fn late_callback(
    owner: &DatabaseConnection,
    service: Arc<IncomingAgent>,
    hosting: Arc<webdock_hosting::HostingService>,
    agent: webdock_hosting::AgentIdentity,
    claim: ConfigurationClaim,
    completion: bool,
) {
    let domain = claim.spec.snapshot.domains[0].id.clone();
    let resource = ConfigurationResource {
        kind: ConfigurationResourceKind::Domain,
        id: domain.clone(),
    };
    let request_sha256 = webdock_agent_protocol::mail::fingerprint(&resource).unwrap();
    let applied = ConfigurationCheckpoint {
        lease: claim.lease.clone(),
        runtime_id: "normal-runtime".into(),
        phase: ConfigurationCheckpointPhase::Applied {
            resource: resource.clone(),
            request_sha256: request_sha256.clone(),
            provider_id: format!("domain-{domain}"),
        },
    };
    let receipt = if completion {
        Some(prepare(&service, &hosting, &agent, &claim).await)
    } else {
        for phase in [
            ConfigurationCheckpointPhase::Bound {},
            ConfigurationCheckpointPhase::Handoff {
                resource,
                request_sha256,
            },
        ] {
            service
                .checkpoint_configuration(
                    hosting.begin_authenticated_agent(&agent).await.unwrap(),
                    &agent,
                    &claim.spec,
                    ConfigurationCheckpoint {
                        lease: claim.lease.clone(),
                        runtime_id: "normal-runtime".into(),
                        phase,
                    },
                )
                .await
                .unwrap();
        }
        None
    };
    let block = owner.begin().await.unwrap();
    sql(
        &block,
        "SELECT id FROM webdock_mail.domain WHERE id=$1 FOR UPDATE",
        vec![domain.clone().into()],
    )
    .await;
    sql(owner,"UPDATE webdock_mail.configuration_operation SET lease_until=clock_timestamp()+interval '2 seconds' WHERE id=$1",vec![claim.spec.operation_id.clone().into()]).await;
    let task_service = service.clone();
    let task_hosting = hosting.clone();
    let task_agent = agent.clone();
    let task_spec = claim.spec.clone();
    let task_applied = applied.clone();
    let task_receipt = receipt.clone();
    let mut task = tokio::spawn(async move {
        let tx = task_hosting
            .begin_authenticated_agent(&task_agent)
            .await
            .unwrap();
        match task_receipt {
            Some(receipt) => {
                task_service
                    .complete_configuration(tx, &task_agent, &task_spec, receipt)
                    .await
            }
            None => {
                task_service
                    .checkpoint_configuration(tx, &task_agent, &task_spec, task_applied)
                    .await
            }
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(1),async{loop{let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND pid<>pg_backend_pid()) AS blocked")).await.unwrap().unwrap();if row.try_get::<bool>("","blocked").unwrap(){break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3),async{loop{let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT lease_until<clock_timestamp() AS expired FROM webdock_mail.configuration_operation WHERE state='running'")).await.unwrap().unwrap();if row.try_get::<bool>("","expired").unwrap(){break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}).await.unwrap();
    block.rollback().await.unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), &mut task)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(result, Err(webdock_mail::incoming::Error::Conflict)),
        "expired projection was committed"
    );
    let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT d.state,d.provider_id,c.applied_revision,n.receipt_sha256 FROM webdock_mail.domain d JOIN webdock_mail.configuration c USING(customer_id) JOIN webdock_mail.native_configuration_operation n USING(customer_id)")).await.unwrap().unwrap();
    assert_eq!(row.try_get::<String>("", "state").unwrap(), "pending");
    assert_eq!(row.try_get::<i64>("", "applied_revision").unwrap(), 0);
    assert!(
        row.try_get::<Option<String>>("", "receipt_sha256")
            .unwrap()
            .is_none()
    );
    if !completion {
        assert!(
            row.try_get::<Option<String>>("", "provider_id")
                .unwrap()
                .is_none()
        );
    }
    let tx = hosting.begin_authenticated_agent(&agent).await.unwrap();
    match receipt {
        Some(receipt) => service
            .complete_configuration(tx, &agent, &claim.spec, receipt)
            .await
            .unwrap(),
        None => service
            .checkpoint_configuration(tx, &agent, &claim.spec, applied)
            .await
            .unwrap(),
    };
    let row = owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT c.applied_revision FROM webdock_mail.configuration c WHERE customer_id='123'",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.try_get::<i64>("", "applied_revision").unwrap(), 0);
}

async fn nonoperator_case(
    owner: &DatabaseConnection,
    incoming: &NativeIncoming,
    service: Arc<IncomingAgent>,
    hosting: Arc<webdock_hosting::HostingService>,
    agent: webdock_hosting::AgentIdentity,
    identity: &IdentityService,
    case: Case,
) {
    sql(owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"mustChangePassword\") VALUES('1002','Admin','admin@example.test',true,'user',false)",vec![]).await;
    sql(owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES('admin-session','admin-token','1002',now()+interval '1 hour',now())",vec![]).await;
    sql(owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('original-admin','124','1002','admin',now())",vec![]).await;
    let cookies = CookiePolicy::new(false, "incoming-agent-cookie-key-atleast32").unwrap();
    let cookie = format!(
        "{}={}",
        cookies.name(),
        cookies.sign("admin-token").unwrap()
    );
    let admin = identity
        .authenticate_cookie(&cookie)
        .await
        .unwrap()
        .unwrap();
    incoming
        .manage_mailboxes(
            &admin,
            &StoredAuthority::LocalSession {},
            "123",
            MailboxCommand::DomainAttach {
                sender_domain_id: "301".into(),
            },
        )
        .await
        .unwrap();
    if case == Case::MemberSwap {
        sql(
            owner,
            "DELETE FROM webdock_auth.member WHERE id='original-admin'",
            vec![],
        )
        .await;
        sql(owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('replacement-admin','124','1002','admin',now())",vec![]).await;
        assert!(
            service
                .claim_configuration(
                    hosting.begin_authenticated_agent(&agent).await.unwrap(),
                    &agent
                )
                .await
                .unwrap()
                .is_none(),
            "a new membership revived an old admission"
        );
        return;
    }
    let domain = incoming
        .directory(&admin, &StoredAuthority::LocalSession {}, "123")
        .await
        .unwrap()
        .domains[0]
        .id
        .clone();
    let mailbox = incoming
        .manage_mailboxes(
            &admin,
            &StoredAuthority::LocalSession {},
            "123",
            MailboxCommand::MailboxCreate {
                domain_id: domain,
                local_part: "administrator".into(),
                name: "Admin".into(),
                quota_bytes: 1048576,
            },
        )
        .await
        .unwrap()
        .id
        .unwrap();
    incoming
        .manage_mailboxes(
            &admin,
            &StoredAuthority::LocalSession {},
            "123",
            MailboxCommand::Grant {
                mailbox_id: mailbox,
                subject: "1002".into(),
            },
        )
        .await
        .unwrap();
    let block = owner.begin().await.unwrap();
    sql(
        &block,
        "SELECT id FROM webdock_auth.member WHERE id='original-admin' FOR UPDATE",
        vec![],
    )
    .await;
    let mut task = tokio::spawn(async move {
        service
            .claim_configuration(
                hosting.begin_authenticated_agent(&agent).await.unwrap(),
                &agent,
            )
            .await
    });
    let waiting=tokio::time::timeout(std::time::Duration::from_secs(1),async{loop{let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND pid<>pg_backend_pid()) AS blocked")).await.unwrap().unwrap();if row.try_get::<bool>("","blocked").unwrap(){break;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}}).await;
    if waiting.is_err() {
        block.rollback().await.unwrap();
        task.abort();
        let _ = task.await;
        panic!("worker never waited for original membership");
    }
    let ordered=block.execute_unprepared("SELECT customer_id FROM webdock_mail.configuration WHERE customer_id='123' FOR UPDATE NOWAIT").await.is_ok();
    if !ordered {
        block.rollback().await.unwrap();
        task.abort();
        let _ = task.await;
        panic!("worker locked configuration before its original member fence");
    }
    sql(
        &block,
        "DELETE FROM webdock_auth.member WHERE id='original-admin'",
        vec![],
    )
    .await;
    block.commit().await.unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), &mut task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(result.is_none());
    let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*) AS count FROM webdock_mail.native_configuration_operation WHERE request_sha256 IS NOT NULL")).await.unwrap().unwrap();
    assert_eq!(row.try_get::<i64>("", "count").unwrap(), 0);
}
