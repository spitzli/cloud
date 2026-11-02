//! Full owned-schema launch/exchange/inspection with native identity and restricted roles.
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::FutureExt;
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TransactionTrait,
};
use serde_json::json;
use std::sync::Arc;
use webdock_browser_session::StoredAuthority;
use webdock_identity::{CookiePolicy, IdentityService};
use webdock_mail::{
    keys::{KeyChange, KeyCommand},
    secrets::MailCipher,
    webmail::{WebmailConfig, WebmailDependencies, WebmailService},
};
const KEY: &str = "local-webmail-fixture-context-key-at-least32";
async fn sql(db: &impl ConnectionTrait, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
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
#[ignore = "creates and removes an owned local Webmail database"]
async fn explicit_mailbox_grant_one_use_launch_revocation_and_secret_boundary() {
    let mut url =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    let admin = Database::connect(url.as_str()).await.unwrap();
    let name = format!(
        "webdock_webmail_{}_{}_test",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    );
    admin
        .execute_unprepared(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    url.set_path(&format!("/{name}"));
    let owner = Database::connect(url.as_str()).await.unwrap();
    let result=std::panic::AssertUnwindSafe(async{
        webdock_persistence::migrations::bootstrap(&owner,&name,KEY).await.unwrap();
        sql(&owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES('alice','Alice','alice@example.invalid',true,'user',false,false),('operator','Operator','operator@example.invalid',true,'operator',true,false)",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES('alice-session','alice-token','alice',now()+interval '1 hour',now()),('operator-session','operator-token','operator',now()+interval '1 hour',now())",vec![]).await;
        let seed=owner.begin().await.unwrap();sql(&seed,"SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"123\",\"organization_id\":\"124\"}',true)",vec![]).await;sql(&seed,"INSERT INTO webdock_admin.customers(id,name) VALUES('123','Webmail fixture')",vec![]).await;seed.commit().await.unwrap();
        sql(&owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('member-alice','124','alice','member',now())",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":true}')",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('mail','builtin','enabled','enabled','test-only')",vec![]).await;
        sql(&owner,"INSERT INTO webdock_mail.service(customer_id,instance_key,host_id,desired_enabled,state,revision,applied_revision) VALUES('123','local-fixture','200',true,'ready',1,1)",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.mail_sender_domain(id,customer_id,domain,token,status) VALUES('301','123','example.test','test-only','ownership_verified')",vec![]).await;
        sql(&owner,"INSERT INTO webdock_mail.domain(id,customer_id,sender_domain_id,name,provider_id,state) VALUES('302','123','301','example.test','domain1','ready')",vec![]).await;
        sql(&owner,"INSERT INTO webdock_mail.mailbox(id,customer_id,domain_id,local_part,name,quota_bytes,provider_id,state) VALUES('303','123','302','alice','Alice mailbox',1048576,'account-a','ready')",vec![]).await;
        let encryption=STANDARD.encode([29u8;32]);let sealed=MailCipher::new(&encryption).unwrap().seal(&json!({"username":"native-alice","password":"retained-fixture-password"}),"identity:123:304").unwrap();
        sql(&owner,"INSERT INTO webdock_mail.identity(id,customer_id,subject,provider_id,encrypted_credentials,state) VALUES('304','123','alice','principal1',$1,'ready')",vec![sealed.into()]).await;
        sql(&owner,"INSERT INTO webdock_mail.mailbox_grant(customer_id,mailbox_id,subject,enabled,revision,applied_revision,member_id) VALUES('123','303','alice',true,1,1,'member-alice')",vec![]).await;
        let auth=role("AUTH_DATABASE_URL",&name,&url).await;let business=role("DATABASE_URL",&name,&url).await;let worker=role("WEBDOCK_PLUGIN_WORKER_DATABASE_URL",&name,&url).await;
        let cookies=CookiePolicy::new(false,"local-webmail-cookie-fixture-key-at-least32").unwrap();let alice_cookie=format!("{}={}",cookies.name(),cookies.sign("alice-token").unwrap());let operator_cookie=format!("{}={}",cookies.name(),cookies.sign("operator-token").unwrap());
        let identity=Arc::new(IdentityService::new(auth.clone(),cookies));let alice=identity.authenticate_cookie(&alice_cookie).await.unwrap().unwrap();let operator=identity.authenticate_cookie(&operator_cookie).await.unwrap().unwrap();
        let ids=Arc::new(webdock_persistence::IdAllocator::new(role("DATABASE_URL",&name,&url).await,webdock_ids::Identity::platform(0,2).unwrap()));
        let dependencies=WebmailDependencies{auth:auth.clone(),business:business.clone(),worker:worker.clone(),identity:identity.clone(),browser:None,ids};
        let keys=webdock_mail::keys::NativeKeys::new(dependencies.clone(),KEY,true).await.unwrap();
        let outgoing=owner.begin().await.unwrap();sql(&outgoing,"SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"125\",\"organization_id\":\"126\"}',true)",vec![]).await;sql(&outgoing,"INSERT INTO webdock_admin.customers(id,name) VALUES('125','Outgoing-only fixture')",vec![]).await;outgoing.commit().await.unwrap();
        sql(&owner,"INSERT INTO webdock_auth.mail_sender_domain(id,customer_id,domain,token,status) VALUES('501','125','outgoing.test','test-only','ownership_verified')",vec![]).await;
        let create = KeyCommand::Create { label: "Sending".into(), domain_id: Some("501".into()) };
        assert!(keys.manage(&alice,&StoredAuthority::LocalSession {},"125",create.clone()).await.is_err());
        let key=keys.manage(&operator,&StoredAuthority::LocalSession {},"125",create).await.unwrap();
        let KeyChange::Created { id: key_id, token } = key else { panic!("Creation must return the one-time token") };
        let token = token.as_str(); let key_id = key_id.as_str();
        assert_eq!(keys.authenticate(token,Some("sender@outgoing.test")).await.unwrap().customer_id,"125");
        assert!(keys.authenticate(token,Some("sender@other.test")).await.is_err());
        let no_engine=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*)::bigint AS count FROM webdock_mail.service WHERE customer_id='125'")).await.unwrap().unwrap();assert_eq!(no_engine.try_get::<i64>("","count").unwrap(),0);
        sql(&owner,"UPDATE webdock_auth.mail_sender_domain SET deleted_at=now() WHERE id='501'",vec![]).await;assert!(keys.authenticate(token,None).await.is_err());
        sql(&owner,"UPDATE webdock_auth.mail_sender_domain SET deleted_at=NULL WHERE id='501'",vec![]).await;
        assert!(keys.authorize_id("125",key_id,Some("sender@outgoing.test")).await.is_ok());
        keys.manage(&operator,&StoredAuthority::LocalSession {},"125",KeyCommand::Revoke { key_id: key_id.into() }).await.unwrap();
        assert!(keys.authenticate(token,None).await.is_err());assert!(keys.authorize_id("125",key_id,None).await.is_err());
        drop(keys);
        let service=WebmailService::new(dependencies,WebmailConfig{enabled:true,gateway_origin:"https://mail.example.test".into(),managed_cluster:"200".into(),rls_secret:KEY.into(),encryption_key:encryption.clone()}).await.unwrap();
        assert!(service.launch(&operator,&StoredAuthority::LocalSession {},"123","303").await.is_err());
        sql(&owner,"INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id,mode) VALUES('operator-session','operator','123','124','owner')",vec![]).await;
        let acting=identity.authenticate_cookie(&operator_cookie).await.unwrap().unwrap();
        let acting_proof=StoredAuthority::local_session(&acting);
        assert!(service.launch(&acting,&acting_proof,"123","303").await.is_err(),"owner still needs explicit mailbox grant");
        let sealed=MailCipher::new(&encryption).unwrap().seal(&json!({"username":"native-operator","password":"fixture-only-password"}),"identity:123:305").unwrap();
        sql(&owner,"INSERT INTO webdock_mail.identity(id,customer_id,subject,provider_id,encrypted_credentials,state) VALUES('305','123','operator','principal2',$1,'ready')",vec![sealed.into()]).await;
        sql(&owner,"INSERT INTO webdock_mail.mailbox_grant(customer_id,mailbox_id,subject,enabled,revision,applied_revision,operator_grant) VALUES('123','303','operator',true,1,1,true)",vec![]).await;
        let scoped_launch=service.launch(&acting,&acting_proof,"123","303").await.unwrap();
        let scoped_session=service.exchange(&scoped_launch.code).await.unwrap();
        assert!(service.inspect(&scoped_session.token).await.is_ok());
        assert!(service.launch(&acting,&acting_proof,"125","303").await.is_err());
        sql(&owner,"DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id='operator-session'",vec![]).await;
        assert!(service.inspect(&scoped_session.token).await.is_err());
        sql(&owner,"INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id,mode) VALUES('operator-session','operator','123','124','owner')",vec![]).await;
        assert!(service.inspect(&scoped_session.token).await.is_err(),"same-customer reentry never revives a prior session");
        assert!(service.launch(&acting,&StoredAuthority::LocalSession {},"123","303").await.is_err());
        sql(&owner,"DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id='operator-session'",vec![]).await;
        let launch=service.launch(&alice,&StoredAuthority::LocalSession {},"123","303").await.unwrap();assert!(!serde_json::to_string(&launch).unwrap().contains("password"));
        assert!(owner.execute_unprepared("UPDATE webdock_mail.webmail_launch SET native_authority='{}' WHERE session_id='alice-session'").await.is_err());
        let exchanged=service.exchange(&launch.code).await.unwrap();assert!(service.exchange(&launch.code).await.is_err());assert!(!serde_json::to_string(&exchanged).unwrap().contains("password"));
        let access=service.inspect(&exchanged.token).await.unwrap();assert_eq!(access.account_id,"account-a");assert_eq!(access.sender_address,"alice@example.test");assert!(access.authorization.starts_with("Basic "));
        use webdock_mail::webmail::{ReserveWrite,FinishWrite,WriteAction,WriteState,WriteResult};
        let request=uuid::Uuid::new_v4();
        let reserve=||ReserveWrite{token:exchanged.token.clone(),request_id:request,action:WriteAction::Submit,resource_id:Some("draft1".into()),payload_hash:"a".repeat(64)};
        let first=service.reserve_write(reserve()).await.unwrap();assert!(first.execute);
        let replay=service.reserve_write(reserve()).await.unwrap();assert!(!replay.execute);assert_eq!(replay.operation_id,first.operation_id);assert!(replay.lease_token.is_none());
        let replacement=ReserveWrite{token:exchanged.token.clone(),request_id:uuid::Uuid::new_v4(),action:WriteAction::Draft,resource_id:Some("draft1".into()),payload_hash:"a".repeat(64)};
        assert!(service.reserve_write(replacement).await.is_err());
        let finish=||FinishWrite{token:exchanged.token.clone(),operation_id:first.operation_id.clone(),lease_token:first.lease_token.unwrap(),state:WriteState::Succeeded,result:Some(WriteResult{id:Some("submission1".into()),state:Some("state1".into())})};
        service.finish_write(finish()).await.unwrap();service.finish_write(finish()).await.unwrap();
        let mut altered=finish();altered.result.as_mut().unwrap().id=Some("substitution".into());assert!(service.finish_write(altered).await.is_err());
        let replay=service.reserve_write(reserve()).await.unwrap();assert!(!replay.execute);assert_eq!(replay.result.unwrap()["id"],"submission1");
        sql(&owner,"UPDATE webdock_auth.plugin_installation SET state='draining' WHERE id='mail'",vec![]).await;
        assert!(service.inspect(&exchanged.token).await.is_ok());assert!(service.reserve_write(reserve()).await.is_err());
        sql(&owner,"UPDATE webdock_auth.plugin_installation SET state='enabled' WHERE id='mail'",vec![]).await;
        sql(&owner,"UPDATE webdock_mail.mailbox_grant SET revision=2,applied_revision=2 WHERE mailbox_id='303'",vec![]).await;assert!(service.inspect(&exchanged.token).await.is_err());
        let launch=service.launch(&alice,&StoredAuthority::LocalSession {},"123","303").await.unwrap();let second=service.exchange(&launch.code).await.unwrap();service.revoke(&second.token).await.unwrap();assert!(service.inspect(&second.token).await.is_err());
        let launch=service.launch(&alice,&StoredAuthority::LocalSession {},"123","303").await.unwrap();let third=service.exchange(&launch.code).await.unwrap();sql(&owner,"UPDATE webdock_auth.\"user\" SET banned=true WHERE id='alice'",vec![]).await;assert!(service.inspect(&third.token).await.is_err());
        drop(service);drop(identity);auth.close().await.unwrap();business.close().await.unwrap();worker.close().await.unwrap();
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
