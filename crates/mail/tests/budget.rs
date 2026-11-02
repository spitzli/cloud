//! Isolated fresh database, real retained schema and restricted worker RLS.
use futures_util::FutureExt;
use hmac::{Hmac, Mac};
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait,
};
use serde_json::json;
use sha2::Sha256;
use webdock_mail::budget::*;
const KEY: &str = "local-mail-budget-fixture-context-key-32bytes";
async fn sql(db: &impl ConnectionTrait, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}
async fn tx(db: &DatabaseConnection) -> DatabaseTransaction {
    let tx = db.begin().await.unwrap();
    let raw=json!({"v":1,"aud":"webdock_worker_runtime","exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"system","purpose":"mail-send-gateway"}}).to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(KEY.as_bytes()).unwrap();
    mac.update(raw.as_bytes());
    sql(&tx,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",vec![raw.into(),hex::encode(mac.finalize().into_bytes()).into()]).await;
    tx
}
#[tokio::test]
#[ignore = "creates and removes an owned local Mail database"]
async fn concurrent_quota_reservation_suspension_and_month_transfer() {
    let mut owner_url =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert_eq!(owner_url.host_str(), Some("127.0.0.1"));
    assert_eq!(owner_url.path(), "/webdock_rls_test");
    assert!(owner_url.query().is_none());
    let admin = Database::connect(owner_url.as_str()).await.unwrap();
    let name = format!(
        "webdock_mail_{}_{}_test",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    );
    admin
        .execute_unprepared(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    owner_url.set_path(&format!("/{name}"));
    let owner = Database::connect(owner_url.as_str()).await.unwrap();
    let result=std::panic::AssertUnwindSafe(async{
        webdock_persistence::migrations::bootstrap(&owner,&name,KEY).await.unwrap();
        let mut worker_url=url::Url::parse(&std::env::var("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").unwrap()).unwrap();
        assert_eq!(worker_url.host_str(),owner_url.host_str());assert_eq!(worker_url.port(),owner_url.port());worker_url.set_path(&format!("/{name}"));
        let worker=Database::connect(worker_url.as_str()).await.unwrap();
        let seed=owner.begin().await.unwrap();
        sql(&seed,"SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"123\",\"organization_id\":\"124\"}',true)",vec![]).await;
        sql(&seed,"INSERT INTO webdock_admin.customers(id,name) VALUES('123','Mail fixture')",vec![]).await;seed.commit().await.unwrap();
        sql(&owner,"INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":true}')",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.tenant_subscription(customer_id,name,description,base,extras,revision,updated_by) VALUES('123','Mail fixture','Disposable','{\"mailMessages\":3}','{}',1,'fixture')",vec![]).await;
        let mut tasks=tokio::task::JoinSet::new();
        for id in ["1001","1002","1003","1004"]{let db=worker.clone();tasks.spawn(async move{let t=tx(&db).await;let result=reserve(&t,"123",id,1).await; if result.is_ok(){t.commit().await.unwrap();}else{t.rollback().await.unwrap();} (id,result.is_ok())});}
        let mut accepted=Vec::new();while let Some(v)=tasks.join_next().await{let(id,ok)=v.unwrap();if ok{accepted.push(id);}}
        assert_eq!(accepted.len(),3);let id=accepted[0];
        let t=tx(&worker).await;assert!(!reserve(&t,"123",id,1).await.unwrap().created);assert!(reserve(&t,"123",id,2).await.is_err());t.rollback().await.unwrap();
        let t=tx(&worker).await;settle(&t,"123",id,Settlement::Accepted).await.unwrap();settle(&t,"123",id,Settlement::Accepted).await.unwrap();assert!(settle(&t,"123",id,Settlement::Released).await.is_err());t.rollback().await.unwrap();
        let t=tx(&worker).await;settle(&t,"123",id,Settlement::Released).await.unwrap();t.commit().await.unwrap();
        let t=tx(&worker).await;assert!(reserve(&t,"123","1005",1).await.unwrap().created);t.commit().await.unwrap();
        // Incoming activation changes route and must not let queued outgoing work bypass it.
        sql(&owner,"INSERT INTO webdock_mail.service(customer_id,instance_key,host_id,desired_enabled,state,revision,applied_revision) VALUES('123','fixture-instance','fixture-host',false,'suspended',1,1)",vec![]).await;
        let t=tx(&worker).await;assert!(reserve(&t,"123","1005",1).await.is_err());t.rollback().await.unwrap();
        sql(&owner,"UPDATE webdock_mail.service SET desired_enabled=true,state='ready' WHERE customer_id='123'",vec![]).await;
        let t=tx(&worker).await;assert!(reserve(&t,"123","1005",1).await.is_err());t.rollback().await.unwrap();
        sql(&owner,"UPDATE webdock_auth.tenant_subscription SET base='{\"mailMessages\":null}' WHERE customer_id='123'",vec![]).await;
        let t=tx(&worker).await;let new=reserve(&t,"123","1006",2).await.unwrap();assert_eq!(new.path,SendPath::Stalwart{service_revision:"1".into()});t.commit().await.unwrap();
        sql(&owner,"INSERT INTO webdock_mail.month_usage(customer_id,period_start,reserved) VALUES('123','2000-01-01',2)",vec![]).await;
        sql(&owner,"UPDATE webdock_mail.month_usage SET reserved=reserved-2 WHERE customer_id='123' AND period_start=date_trunc('month',now() AT TIME ZONE 'UTC')::date",vec![]).await;
        sql(&owner,"UPDATE webdock_mail.budget_reservation SET period_start='2000-01-01' WHERE reference_id='1006'",vec![]).await;
        let t=tx(&worker).await;assert!(!reserve(&t,"123","1006",2).await.unwrap().created);settle(&t,"123","1006",Settlement::Accepted).await.unwrap();t.commit().await.unwrap();
        let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT reserved FROM webdock_mail.month_usage WHERE customer_id='123' AND period_start='2000-01-01'")).await.unwrap().unwrap();assert_eq!(row.try_get::<i64>("","reserved").unwrap(),0);
        worker.close().await.unwrap();
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
