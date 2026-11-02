use futures_util::FutureExt;
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TransactionTrait,
};
use serde_json::json;
use std::sync::Arc;
use webdock_mail::{
    MailDependencies,
    outbox::*,
    submissions::{QueueCommand, Submissions},
};
const KEY: &str = "mail-submissions-fixture-context-at-least32";
async fn role(key: &str, name: &str, owner: &url::Url) -> DatabaseConnection {
    let mut url = url::Url::parse(&std::env::var(key).unwrap()).unwrap();
    assert_eq!(url.host_str(), owner.host_str());
    assert_eq!(url.port(), owner.port());
    url.set_path(&format!("/{name}"));
    Database::connect(url.as_str()).await.unwrap()
}
#[tokio::test]
#[ignore = "creates and removes one owned local submission database"]
async fn native_queue_settles_budget_exactly_once_and_preserves_unknown_payloads() {
    use sha2::{Digest, Sha256};
    let mut url =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert!(matches!(url.host_str(), Some("localhost" | "127.0.0.1")));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    let admin = Database::connect(url.as_str()).await.unwrap();
    let name = format!("webdock_mail_queue_{}_test", uuid::Uuid::new_v4().simple());
    admin
        .execute_unprepared(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    url.set_path(&format!("/{name}"));
    let result=std::panic::AssertUnwindSafe(async{
        let owner=Database::connect(url.as_str()).await.unwrap();webdock_persistence::migrations::bootstrap(&owner,&name,KEY).await.unwrap();
        let seed=owner.begin().await.unwrap();seed.execute_unprepared("SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"123\",\"organization_id\":\"124\"}',true)").await.unwrap();seed.execute_unprepared("INSERT INTO webdock_admin.customers(id,name) VALUES('123','Queue fixture')").await.unwrap();seed.commit().await.unwrap();
        owner.execute_unprepared("INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('mail','builtin','enabled','enabled','fixture'); INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":true}')").await.unwrap();
        let token=format!("wd_mail_{}","t".repeat(43));owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_mail.api_key(id,customer_id,label,token_hash,key_suffix,created_by) VALUES('456','123','fixture',$1,'tttt','fixture')",[hex::encode(Sha256::digest(&token)).into()])).await.unwrap();
        let auth=role("AUTH_DATABASE_URL",&name,&url).await;let business=role("DATABASE_URL",&name,&url).await;let worker=role("WEBDOCK_PLUGIN_WORKER_DATABASE_URL",&name,&url).await;
        let identity=Arc::new(webdock_identity::IdentityService::new(auth.clone(),webdock_identity::CookiePolicy::new(false,"fixture-session-key-at-least32-characters").unwrap()));let ids=Arc::new(webdock_persistence::IdAllocator::new(role("DATABASE_URL",&name,&url).await,webdock_ids::Identity::platform(0,2).unwrap()));
        let service=Submissions::new(MailDependencies{auth,business,worker,identity,browser:None,ids},KEY,true).await.unwrap();
        owner.execute_unprepared("INSERT INTO webdock_mail.month_usage(customer_id,period_start,reserved,accepted) VALUES('123',date_trunc('month',now() AT TIME ZONE 'UTC')::date,3,0)").await.unwrap();
        for (id,state)in [("789","queued"),("790","staging"),("791","queued")]{
            let reference=uuid::Uuid::new_v4().to_string();let upload=uuid::Uuid::new_v4().to_string();
            owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_mail.send_job(id,customer_id,source_id,idempotency_key,request_hash,payload_ref,payload_bytes,route,envelope,state,lease_token,lease_until) VALUES($1,'123','key:456',$1,$2,$3::uuid,100,'{\"kind\":\"direct\"}','{\"from\":\"sender@example.test\",\"to\":[\"target@example.test\"]}',$4,$5::uuid,now()+interval '2 minutes')",vec![id.into(),"a".repeat(64).into(),reference.into(),state.into(),upload.into()])).await.unwrap();
            owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_mail.budget_reservation(reference_id,customer_id,units,period_start,send_path,state) VALUES($1,'123',1,date_trunc('month',now() AT TIME ZONE 'UTC')::date,'{\"kind\":\"direct\"}','reserved')",[id.into()])).await.unwrap();
        }
        let status=service.execute(QueueCommand::Status{token:token.clone(),id:"789".into(),cancel:false}).await.unwrap();assert_eq!(status["state"],"queued");
        let first:SendLease=serde_json::from_value(service.execute(QueueCommand::Claim {}).await.unwrap()).unwrap();assert_eq!(first.id,"789");
        owner.execute_unprepared("UPDATE webdock_mail.send_job SET state='handoff' WHERE id='789'").await.unwrap();
        assert_eq!(service.execute(QueueCommand::Finish{lease:first.clone(),result:Receipt::Unknown {}}).await.unwrap(),true);
        assert_eq!(service.execute(QueueCommand::Status{token:token.clone(),id:"789".into(),cancel:true}).await.unwrap()["state"],"unknown");
        let accepted=Receipt::Accepted{provider_id:"provider_1".into()};assert_eq!(service.execute(QueueCommand::Finish{lease:first.clone(),result:accepted.clone()}).await.unwrap(),true);assert_eq!(service.execute(QueueCommand::Finish{lease:first,result:accepted}).await.unwrap(),true);
        owner.execute_unprepared("UPDATE webdock_mail.send_job SET lease_until=now()-interval '1 second' WHERE id='790'").await.unwrap();
        let second:SendLease=serde_json::from_value(service.execute(QueueCommand::Claim {}).await.unwrap()).unwrap();assert_eq!(second.id,"791");
        assert_eq!(service.execute(QueueCommand::ReviewPreparation{lease:second}).await.unwrap(),true);
        assert_eq!(service.execute(QueueCommand::Status{token:token.clone(),id:"791".into(),cancel:true}).await.unwrap()["state"],"cancelled");
        assert!(service.execute(QueueCommand::Claim {}).await.unwrap().is_null());
        let usage=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT reserved,accepted FROM webdock_mail.month_usage WHERE customer_id='123'")).await.unwrap().unwrap();assert_eq!(usage.try_get::<i64>("","reserved").unwrap(),0);assert_eq!(usage.try_get::<i64>("","accepted").unwrap(),1);
        owner.execute_unprepared("UPDATE webdock_mail.send_job SET finished_at=now()-interval '2 minutes' WHERE customer_id='123'").await.unwrap();
        let cleanup=service.execute(QueueCommand::Cleanup {}).await.unwrap();assert_eq!(cleanup.as_array().unwrap().len(),3);
        service.execute(QueueCommand::CleanupDone{items:serde_json::from_value(cleanup).unwrap()}).await.unwrap();assert_eq!(service.execute(QueueCommand::Cleanup {}).await.unwrap(),json!([]));
        owner.execute_unprepared("UPDATE webdock_mail.api_key SET revoked_at=now() WHERE id='456'").await.unwrap();assert!(service.execute(QueueCommand::Status{token,id:"789".into(),cancel:false}).await.is_err());
        drop(service);owner.close().await.unwrap();
    }).catch_unwind().await;
    admin
        .execute_unprepared(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
