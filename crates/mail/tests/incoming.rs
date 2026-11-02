//! Incoming desired-state administration uses real restricted roles; fixture-only observations
//! are not evidence of a running mail engine.
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use futures_util::FutureExt;
use http_body_util::BodyExt;
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TransactionTrait,
};
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;
use webdock_browser_session::StoredAuthority;
use webdock_identity::{CookiePolicy, IdentityService};
use webdock_mail::{
    MailDependencies,
    incoming::{IncomingConfig, MailboxCommand, NativeIncoming, ServiceCommand, ServiceState},
};

const KEY: &str = "owned-native-incoming-fixture-key-at-least32";
fn app(
    incoming: Arc<NativeIncoming>,
    identity: Arc<IdentityService>,
    db: DatabaseConnection,
    feature: &str,
) -> Router {
    let config =
        webdock_core::config::PlatformConfig::from_map(&std::collections::HashMap::from([(
            "WEBDOCK_FEATURES".into(),
            json!({"mail":feature}).to_string(),
        )]))
        .unwrap();
    webdock_control_plane::http::build_app_with_domains(
        config,
        db,
        webdock_control_plane::http::Services {
            identity: Some(identity),
            mail: Some(Arc::new(
                webdock_control_plane::mail::MailEndpoints::incoming_only(incoming),
            )),
            ..Default::default()
        },
    )
}
async fn request(
    app: &Router,
    cookie: Option<&str>,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/console/mail")
        .header("host", "localhost:3120")
        .header("origin", "http://localhost:3120")
        .header("content-type", "application/json");
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
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
#[test]
fn incoming_commands_reject_ambiguous_payloads() {
    assert!(
        serde_json::from_value::<MailboxCommand>(json!({
            "action":"mailbox.create","domainID":"123","localPart":"me","name":"Me",
            "quotaBytes":"1048576"
        }))
        .is_ok(),
        "retained form envelopes encode quota as a decimal string"
    );
    for value in [
        json!({"action":"activate","revision":0}),
        json!({"action":"activate","revision":"0","hostID":"200"}),
        json!({"action":"delete","revision":"1"}),
    ] {
        assert!(serde_json::from_value::<ServiceCommand>(value).is_err());
    }
    for value in [
        json!({"action":"grant","mailboxID":"123","subject":"124","operatorGrant":true}),
        json!({"action":"mailbox.create","domainID":"123","localPart":"me","name":"Me","quotaBytes":1.5}),
        json!({"action":"configuration.retry","revision":"1","force":true}),
    ] {
        assert!(serde_json::from_value::<MailboxCommand>(value).is_err());
    }
}

#[tokio::test]
#[ignore = "creates and removes an owned local incoming Mail database"]
async fn incoming_admission_retention_revision_and_live_grant_boundaries() {
    let mut url =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    let admin = Database::connect(url.as_str()).await.unwrap();
    let name = format!(
        "webdock_incoming_{}_{}_test",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    );
    admin
        .execute_unprepared(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    url.set_path(&format!("/{name}"));
    let owner = Database::connect(url.as_str()).await.unwrap();
    let result = std::panic::AssertUnwindSafe(async {
        webdock_persistence::migrations::bootstrap(&owner,&name,KEY).await.unwrap();
        sql(&owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES('1001','Operator','operator@example.invalid',true,'operator',true,false),('1002','Admin','admin@example.invalid',true,'user',false,false),('1003','Member','member@example.invalid',true,'user',false,false)",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES('operator-session','operator-token','1001',now()+interval '1 hour',now()),('admin-session','admin-token','1002',now()+interval '1 hour',now()),('member-session','member-token','1003',now()+interval '1 hour',now())",vec![]).await;
        for (customer, organization) in [("123","124"),("125","126")] {
            let tx=owner.begin().await.unwrap();
            sql(&tx,"SELECT set_config('webdock.organization_allocation',$1,true)",vec![json!({"customer_id":customer,"organization_id":organization}).to_string().into()]).await;
            sql(&tx,"INSERT INTO webdock_admin.customers(id,name) VALUES($1,'Incoming fixture')",vec![customer.into()]).await;
            tx.commit().await.unwrap();
        }
        sql(&owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('member-admin','124','1002','admin',now()),('member-ordinary','124','1003','member',now())",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":true}')",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('mail','builtin','enabled','enabled','test-only')",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,verified,capacity) VALUES('200','Owned Mail fixture','managed','eu','fixture',true,$1)",vec![json!({"apps":1,"cpuMillicores":500,"memoryBytes":1073741824u64,"volumeBytes":10737418240u64,"ephemeralBytes":268435456,"replicasPerApp":1,"concurrentDeployments":1}).into()]).await;
        sql(&owner,"INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_seen,observation) VALUES('200','fixture',1,now(),$1)",vec![json!({"version":"v1.36.5+k3s1","capabilities":{"nativeMail":{"version":1},"storageVersion":1}}).into()]).await;
        let auth=role("AUTH_DATABASE_URL",&name,&url).await;
        let business=role("DATABASE_URL",&name,&url).await;
        let worker=role("WEBDOCK_PLUGIN_WORKER_DATABASE_URL",&name,&url).await;
        let cookies=CookiePolicy::new(false,"incoming-cookie-fixture-key-at-least32").unwrap();
        let cookie = |token| format!("{}={}",cookies.name(),cookies.sign(token).unwrap());
        let operator_cookie=cookie("operator-token"); let admin_cookie=cookie("admin-token");let member_cookie=cookie("member-token");
        let identity=Arc::new(IdentityService::new(auth.clone(),cookies));
        let operator=identity.authenticate_cookie(&operator_cookie).await.unwrap().unwrap();
        let tenant_admin=identity.authenticate_cookie(&admin_cookie).await.unwrap().unwrap();
        let member=identity.authenticate_cookie(&member_cookie).await.unwrap().unwrap();
        let ids=Arc::new(webdock_persistence::IdAllocator::new(role("DATABASE_URL",&name,&url).await,webdock_ids::Identity::platform(0,2).unwrap()));
        let dependencies=MailDependencies{auth:auth.clone(),business:business.clone(),worker:worker.clone(),identity:identity.clone(),browser:None,ids};
        let keys=webdock_mail::keys::NativeKeys::new(dependencies.clone(),KEY,true).await.unwrap();
        let incoming=Arc::new(NativeIncoming::new(dependencies,IncomingConfig{enabled:true,mailboxes_enabled:true,sending_enabled:true,managed_cluster:"200".into(),rls_secret:KEY.into()}).await.unwrap());
        let http=app(incoming.clone(),identity.clone(),auth.clone(),"enabled");
        let authority=StoredAuthority::LocalSession {};
        let view=incoming.service(&operator,&authority,"123").await.unwrap();
        assert_eq!(view.service.state,ServiceState::Disabled);
        assert_eq!(view.service.revision,"0");
        let read=json!({"action":"getTenantMailService","customerID":"123"});
        assert_eq!(request(&http,None,read.clone()).await.0,StatusCode::UNAUTHORIZED);
        let (status,data)=request(&http,Some(&operator_cookie),read.clone()).await;
        assert_eq!(status,StatusCode::OK);assert_eq!(data["data"]["service"]["state"],"disabled");
        let disabled=app(incoming.clone(),identity.clone(),auth.clone(),"disabled");
        assert_eq!(request(&disabled,Some(&operator_cookie),read).await.0,StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(request(&http,Some(&admin_cookie),json!({"action":"getTenantMailService","customerID":"125"})).await.0,StatusCode::FORBIDDEN);
        assert_eq!(request(&http,Some(&operator_cookie),json!({"action":"manageTenantMailService","customerID":"123","input":{"action":"activate","revision":"0","hostID":"999"}})).await.0,StatusCode::BAD_REQUEST);
        assert_eq!(request(&http,Some(&operator_cookie),json!({"action":"manageTenantMailboxes","customerID":"123","input":{"action":"grant","mailboxID":"303","subject":"1003","operatorGrant":true}})).await.0,StatusCode::BAD_REQUEST);

        assert!(incoming.directory(&tenant_admin,&authority,"123").await.unwrap().mailboxes.is_empty());
        incoming.manage_service(&operator,&authority,"123",ServiceCommand::Suspend{revision:"0".into()}).await.unwrap();
        assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.service").await,0);
        assert!(incoming.service(&member,&authority,"125").await.is_err());
        assert!(incoming.manage_service(&tenant_admin,&authority,"123",ServiceCommand::Activate{revision:"0".into()}).await.is_err());
        sql(&owner,"INSERT INTO webdock_mail.month_usage(customer_id,period_start) VALUES('123',date_trunc('month',now())::date)",vec![]).await;
        sql(&owner,"INSERT INTO webdock_mail.budget_reservation(reference_id,customer_id,period_start,units) VALUES('held-outgoing','123',date_trunc('month',now())::date,1)",vec![]).await;
        assert!(incoming.manage_service(&operator,&authority,"123",ServiceCommand::Activate{revision:"0".into()}).await.is_err());
        assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.service").await,0);
        sql(&owner,"UPDATE webdock_mail.budget_reservation SET state='released' WHERE reference_id='held-outgoing'",vec![]).await;
        sql(&owner,"INSERT INTO webdock_mail.send_job(id,customer_id,source_id,idempotency_key,request_hash,payload_ref,payload_bytes,route,state) VALUES('901','123','fixture','unknown','fixture','11111111-1111-4111-8111-111111111111',1,'{}','unknown')",vec![]).await;
        assert!(incoming.manage_service(&operator,&authority,"123",ServiceCommand::Activate{revision:"0".into()}).await.is_err());
        sql(&owner,"UPDATE webdock_mail.send_job SET state='cancelled' WHERE id='901'",vec![]).await;
        sql(&owner,"UPDATE webdock_auth.hosting_agent SET last_seen=now()-interval '2 minutes' WHERE cluster_id='200'",vec![]).await;
        assert!(incoming.manage_service(&operator,&authority,"123",ServiceCommand::Activate{revision:"0".into()}).await.is_err());
        assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.service").await,0);
        sql(&owner,"UPDATE webdock_auth.hosting_agent SET last_seen=now() WHERE cluster_id='200'",vec![]).await;
        let activated=incoming.manage_service(&operator,&authority,"123",ServiceCommand::Activate{revision:"0".into()}).await.unwrap();
        assert_eq!(activated.service.state,ServiceState::Pending);
        assert_eq!(activated.service.revision,"1");
        assert_eq!(request(&http,Some(&operator_cookie),json!({"action":"manageTenantMailService","customerID":"123","input":{"action":"suspend","revision":"0"}})).await.0,StatusCode::CONFLICT);

        assert!(!activated.can_suspend || activated.service.enabled);
        assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.instance").await,0);
        assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.operation").await,1);
        assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.native_operation WHERE subject='1001' AND session_id='operator-session' AND request_sha256 IS NULL").await,1);
        incoming.manage_service(&operator,&authority,"123",ServiceCommand::Activate{revision:"0".into()}).await.unwrap();
        assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.operation").await,1);
        assert!(incoming.manage_service(&operator,&authority,"123",ServiceCommand::Suspend{revision:"0".into()}).await.is_err());
        assert!(incoming.manage_service(&operator,&authority,"125",ServiceCommand::Activate{revision:"0".into()}).await.is_err());
        assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.service WHERE customer_id='125'").await,0);
        // Only an actual executor may set readiness. This fixture seeds an observed legacy instance.
        sql(&owner,"UPDATE webdock_mail.service SET state='ready',applied_revision=revision WHERE customer_id='123'",vec![]).await;
        sql(&owner,"INSERT INTO webdock_mail.instance(customer_id,encrypted_credentials,verified_at) VALUES('123','retained-ciphertext',now())",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.mail_sender_domain(id,customer_id,domain,token,status) VALUES('301','123','example.test','test-only','ownership_verified'),('401','125','other.test','test-only','ownership_verified')",vec![]).await;
        assert!(incoming.manage_mailboxes(&member,&authority,"123",MailboxCommand::DomainAttach{sender_domain_id:"301".into()}).await.is_err());
        assert!(incoming.manage_mailboxes(&tenant_admin,&authority,"123",MailboxCommand::DomainAttach{sender_domain_id:"401".into()}).await.is_err());
        incoming.manage_mailboxes(&tenant_admin,&authority,"123",MailboxCommand::DomainAttach{sender_domain_id:"301".into()}).await.unwrap();
        incoming.manage_mailboxes(&tenant_admin,&authority,"123",MailboxCommand::DomainAttach{sender_domain_id:"301".into()}).await.unwrap();
        let directory=incoming.directory(&tenant_admin,&authority,"123").await.unwrap();
        assert_eq!(directory.domains.len(),1);
        assert_eq!(directory.configuration.unwrap().revision,"1");
        let domain_id=directory.domains[0].id.clone();
        let create=MailboxCommand::MailboxCreate{domain_id:domain_id.clone(),local_part:"hello".into(),name:"Hello".into(),quota_bytes:8589934592};
        let (first,second)=tokio::join!(incoming.manage_mailboxes(&tenant_admin,&authority,"123",create.clone()),incoming.manage_mailboxes(&tenant_admin,&authority,"123",create));
        assert_ne!(first.is_ok(),second.is_ok(),"exactly one concurrent mailbox creation wins");
        assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.mailbox").await,1);
        assert!(incoming.manage_mailboxes(&tenant_admin,&authority,"123",MailboxCommand::MailboxCreate{domain_id:domain_id.clone(),local_part:"second".into(),name:"Second".into(),quota_bytes:1048576}).await.is_err());
        let directory=incoming.directory(&tenant_admin,&authority,"123").await.unwrap();
        let mailbox_id=directory.mailboxes[0].id.clone();
        incoming.manage_mailboxes(&tenant_admin,&authority,"123",MailboxCommand::Grant{mailbox_id:mailbox_id.clone(),subject:"1003".into()}).await.unwrap();
        assert!(!incoming.directory(&member,&authority,"123").await.unwrap().mailboxes[0].can_open);
        assert!(incoming.manage_mailboxes(&tenant_admin,&authority,"123",MailboxCommand::Grant{mailbox_id:mailbox_id.clone(),subject:"1001".into()}).await.is_err());
        sql(&owner,"UPDATE webdock_mail.mailbox SET state='ready',provider_id='mailbox-1' WHERE customer_id='123'",vec![]).await;
        sql(&owner,"UPDATE webdock_mail.identity SET state='ready',provider_id='identity-1' WHERE customer_id='123'",vec![]).await;
        sql(&owner,"UPDATE webdock_mail.mailbox_grant SET applied_revision=revision WHERE customer_id='123'",vec![]).await;
        assert!(incoming.directory(&member,&authority,"123").await.unwrap().mailboxes[0].can_open);
        assert!(!incoming.directory(&operator,&authority,"123").await.unwrap().mailboxes[0].can_open);
        let key=keys.manage(&tenant_admin,&authority,"123",webdock_mail::keys::KeyCommand::Create{label:"Retiring key".into(),domain_id:None}).await.unwrap();
        let webdock_mail::keys::KeyChange::Created{id:key_id,..}=key else {panic!("created key expected")};
        sql(&owner,"UPDATE webdock_auth.plugin_installation SET state='draining' WHERE id='mail'",vec![]).await;
        assert!(keys.manage(&tenant_admin,&authority,"123",webdock_mail::keys::KeyCommand::Create{label:"Denied".into(),domain_id:None}).await.is_err());
        assert!(keys.manage(&member,&authority,"123",webdock_mail::keys::KeyCommand::Revoke{key_id:key_id.clone()}).await.is_err());
        keys.manage(&tenant_admin,&authority,"123",webdock_mail::keys::KeyCommand::Revoke{key_id}).await.unwrap();
        assert!(incoming.directory(&member,&authority,"123").await.is_ok());
        assert!(incoming.manage_mailboxes(&tenant_admin,&authority,"123",MailboxCommand::Grant{mailbox_id:mailbox_id.clone(),subject:"1003".into()}).await.is_err());
        incoming.manage_mailboxes(&tenant_admin,&authority,"123",MailboxCommand::Revoke{mailbox_id:mailbox_id.clone(),subject:"1003".into()}).await.unwrap();
        sql(&owner,"UPDATE webdock_auth.plugin_installation SET state='enabled' WHERE id='mail'",vec![]).await;
        incoming.manage_mailboxes(&tenant_admin,&authority,"123",MailboxCommand::Revoke{mailbox_id,subject:"1003".into()}).await.unwrap();
        assert!(incoming.directory(&member,&authority,"123").await.unwrap().mailboxes.is_empty());
        let configuration=incoming.directory(&tenant_admin,&authority,"123").await.unwrap().configuration.unwrap();
        assert!(incoming.manage_mailboxes(&operator,&authority,"123",MailboxCommand::ConfigurationRetry{revision:configuration.revision.clone()}).await.is_err());
        sql(&owner,"UPDATE webdock_mail.configuration_operation SET state='needs_review' WHERE customer_id='123' AND state='pending'",vec![]).await;
        assert!(incoming.manage_mailboxes(&tenant_admin,&authority,"123",MailboxCommand::ConfigurationRetry{revision:configuration.revision.clone()}).await.is_err());
        incoming.manage_mailboxes(&operator,&authority,"123",MailboxCommand::ConfigurationRetry{revision:configuration.revision.clone()}).await.unwrap();
        assert!(incoming.manage_mailboxes(&operator,&authority,"123",MailboxCommand::ConfigurationRetry{revision:configuration.revision}).await.is_err());
        sql(&owner,"UPDATE webdock_admin.customers SET status='archived' WHERE id='123'",vec![]).await;
        assert!(incoming.directory(&tenant_admin,&authority,"123").await.is_err());
        assert!(!incoming.service(&operator,&authority,"123").await.unwrap().can_activate);
        sql(&owner,"UPDATE webdock_auth.plugin_installation SET state='draining' WHERE id='mail'",vec![]).await;
        let suspended=incoming.manage_service(&operator,&authority,"123",ServiceCommand::Suspend{revision:"1".into()}).await.unwrap();
        assert!(!suspended.service.enabled);
        assert_eq!(suspended.service.state,ServiceState::Pending);
        assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.instance WHERE encrypted_credentials='retained-ciphertext'").await,1);
        assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.mailbox").await,1);
        assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.cluster_reservation").await,1);
        let (status,public)=request(&http,Some(&operator_cookie),json!({"action":"getTenantMailService","customerID":"123"})).await;
        assert_eq!(status,StatusCode::OK);assert!(!public.to_string().contains("retained-ciphertext"));
        assert!(!public.to_string().contains("leaseToken"));assert!(!public.to_string().contains("internal_url"));

        sql(&owner,"UPDATE webdock_mail.service SET state='needs_review' WHERE customer_id='123'",vec![]).await;
        sql(&owner,"UPDATE webdock_mail.operation SET state='running' WHERE customer_id='123' AND revision=2",vec![]).await;
        assert!(incoming.manage_service(&operator,&authority,"123",ServiceCommand::Reconcile{revision:"2".into()}).await.is_err());
        sql(&owner,"UPDATE webdock_mail.operation SET state='needs_review' WHERE customer_id='123' AND revision=2",vec![]).await;
        let reconciled=incoming.manage_service(&operator,&authority,"123",ServiceCommand::Reconcile{revision:"2".into()}).await.unwrap();
        assert_eq!(reconciled.service.revision,"3");
        assert!(!reconciled.service.enabled);
        assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.access_event WHERE action IN ('incoming.activate','incoming.suspend','incoming.reconcile')").await,3);
        assert_eq!(count(&owner,"SELECT count(*) FROM webdock_mail.configuration_operation WHERE state='pending'").await,1);
        sql(&owner,"UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 second' WHERE id='admin-session'",vec![]).await;
        assert!(incoming.directory(&tenant_admin,&authority,"123").await.is_err());
        drop(disabled);drop(http);drop(keys);drop(incoming);drop(identity);auth.close().await.unwrap();business.close().await.unwrap();worker.close().await.unwrap();
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
