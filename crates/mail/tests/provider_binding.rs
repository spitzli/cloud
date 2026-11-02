use futures_util::FutureExt;
use hmac::{Hmac, Mac};
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement, TransactionTrait};
use sha2::Sha256;
const KEY: &str = "mail-provider-binding-local-context-at-least32";

#[tokio::test]
#[ignore = "creates and removes a unique owned local provider-binding database"]
async fn transport_adoption_is_explicit_revisioned_and_read_only_to_the_scoped_worker() {
    let mut owner_url =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert!(matches!(
        owner_url.host_str(),
        Some("localhost" | "127.0.0.1")
    ));
    assert_eq!(owner_url.path(), "/webdock_rls_test");
    assert!(owner_url.query().is_none());
    let admin = Database::connect(owner_url.as_str()).await.unwrap();
    let name = format!(
        "webdock_mail_binding_{}_test",
        uuid::Uuid::new_v4().simple()
    );
    admin
        .execute_unprepared(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    owner_url.set_path(&format!("/{name}"));
    let result=std::panic::AssertUnwindSafe(async{
        let owner=Database::connect(owner_url.as_str()).await.unwrap();
        webdock_persistence::migrations::bootstrap(&owner,&name,KEY).await.unwrap();
        let seed=owner.begin().await.unwrap();
        seed.execute_unprepared("SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"123\",\"organization_id\":\"124\"}',true)").await.unwrap();
        seed.execute_unprepared("INSERT INTO webdock_admin.customers(id,name) VALUES('123','Mail provider binding fixture')").await.unwrap();seed.commit().await.unwrap();
        owner.execute_unprepared("INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('mail-relay-fixture','external','enabled','enabled','fixture-only'); INSERT INTO webdock_mail.send_transport(customer_id,mode,revision,state,region) VALUES('123','managed',1,'ready','eu')").await.unwrap();
        let count=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*)::bigint AS n FROM webdock_mail.provider_binding")).await.unwrap().unwrap();assert_eq!(count.try_get::<i64>("","n").unwrap(),0);
        owner.execute_unprepared("INSERT INTO webdock_mail.provider_binding(customer_id,plugin_id,revision,transport_revision) VALUES('123','mail-relay-fixture',1,1)").await.unwrap();
        assert!(owner.execute_unprepared("UPDATE webdock_mail.provider_binding SET revoked_at=now() WHERE customer_id='123'").await.is_err());
        owner.execute_unprepared("UPDATE webdock_mail.provider_binding SET revision=revision+1,revoked_at=now() WHERE customer_id='123'").await.unwrap();
        let mut worker_url=url::Url::parse(&std::env::var("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").unwrap()).unwrap();assert_eq!(worker_url.host_str(),owner_url.host_str());assert_eq!(worker_url.port(),owner_url.port());worker_url.set_path(owner_url.path());
        let worker=Database::connect(worker_url.as_str()).await.unwrap();
        for purpose in [None,Some("mail-webmail-gateway"),Some("mail-send-gateway")]{
            let tx=worker.begin().await.unwrap();
            if let Some(purpose)=purpose{
                let context=serde_json::json!({"v":1,"aud":"webdock_worker_runtime","exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"system","purpose":purpose}}).to_string();let mut mac=Hmac::<Sha256>::new_from_slice(KEY.as_bytes()).unwrap();mac.update(context.as_bytes());
                tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",[context.into(),hex::encode(mac.finalize().into_bytes()).into()])).await.unwrap();
            }
            let count=tx.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*)::bigint AS n FROM webdock_mail.provider_binding")).await.unwrap().unwrap();assert_eq!(count.try_get::<i64>("","n").unwrap(),if purpose==Some("mail-send-gateway"){1}else{0});
            assert!(tx.execute_unprepared("UPDATE webdock_mail.provider_binding SET revision=revision+1,revoked_at=NULL WHERE customer_id='123'").await.is_err());tx.rollback().await.unwrap();
        }
        provider_attempts(&owner,&worker).await;
        worker.close().await.unwrap();owner.close().await.unwrap();
    }).catch_unwind().await;
    admin
        .execute_unprepared(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

async fn sending(worker: &sea_orm::DatabaseConnection) -> sea_orm::DatabaseTransaction {
    let tx = worker.begin().await.unwrap();
    let context=serde_json::json!({"v":1,"aud":"webdock_worker_runtime","exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"system","purpose":"mail-send-gateway"}}).to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(KEY.as_bytes()).unwrap();
    mac.update(context.as_bytes());
    tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",[context.into(),hex::encode(mac.finalize().into_bytes()).into()])).await.unwrap();
    tx
}
async fn provider_attempts(
    owner: &sea_orm::DatabaseConnection,
    worker: &sea_orm::DatabaseConnection,
) {
    let mut grants = Vec::new();
    for (index, job) in ["789", "790", "791", "792", "793"].into_iter().enumerate() {
        let id = uuid::Uuid::new_v4().to_string();
        let queue = uuid::Uuid::new_v4().to_string();
        let payload = uuid::Uuid::new_v4().to_string();
        owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_mail.send_job(id,customer_id,source_id,idempotency_key,request_hash,payload_ref,payload_bytes,route,envelope,state,lease_token,lease_until) VALUES($1,'123','key:456',$1,$2,$3::uuid,100,'{\"kind\":\"direct\"}','{}','handoff',$4::uuid,now()+interval '2 minutes')",vec![job.into(),"a".repeat(64).into(),payload.into(),queue.clone().into()])).await.unwrap();
        let issued = chrono::Utc::now().timestamp() - if index == 2 { 120 } else { 0 };
        let claims=serde_json::json!({"v":1,"purpose":"mail-send","audience":"webdock.mail.runtime.v1","id":id,"jobID":job,"customerID":"123","pluginID":"mail-relay-fixture","iat":issued,"exp":issued+60}).to_string();
        let statement = Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO webdock_mail.provider_lease(id,job_id,customer_id,plugin_id,queue_lease_token,token_sha256,claims_json,issued_at,expires_at) VALUES($1::uuid,$2,'123','mail-relay-fixture',$3::uuid,$4,$5,to_timestamp($6),to_timestamp($7))",
            vec![
                id.clone().into(),
                job.into(),
                queue.into(),
                "b".repeat(64).into(),
                claims.into(),
                (issued as f64).into(),
                ((issued + 60) as f64).into(),
            ],
        );
        if index == 4 {
            owner
                .execute_unprepared(
                    "UPDATE webdock_mail.send_job SET state='queued' WHERE id='793'",
                )
                .await
                .unwrap();
            let tx = sending(worker).await;
            let error = tx.execute_raw(statement).await.unwrap_err();
            assert!(error.to_string().contains("row-level security"));
            tx.rollback().await.unwrap();
            continue;
        }
        let tx = sending(worker).await;
        tx.execute_raw(statement).await.unwrap();
        tx.commit().await.unwrap();
        grants.push(id);
    }
    // Simultaneous callbacks across independent transactions still admit exactly one attempt.
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let worker = worker.clone();
        let id = grants[0].clone();
        tasks.spawn(async move{let tx=sending(&worker).await;let changed=tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_mail.provider_lease SET attempted_at=now() WHERE id=$1::uuid AND attempted_at IS NULL",[id.into()])).await.unwrap().rows_affected();tx.commit().await.unwrap();changed});
    }
    let mut count = 0;
    while let Some(result) = tasks.join_next().await {
        count += result.unwrap();
    }
    assert_eq!(count, 1);
    for assignment in [
        "attempted_at=NULL",
        "token_sha256=repeat('c',64)",
        "claims_json='{}'",
    ] {
        let tx = sending(worker).await;
        assert!(
            tx.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!("UPDATE webdock_mail.provider_lease SET {assignment} WHERE id=$1::uuid"),
                [grants[0].clone().into()]
            ))
            .await
            .is_err()
        );
        tx.rollback().await.unwrap();
    }
    let tx = sending(worker).await;
    tx.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE webdock_mail.provider_lease SET revoked_at=now() WHERE id=$1::uuid",
        [grants[1].clone().into()],
    ))
    .await
    .unwrap();
    tx.commit().await.unwrap();
    owner
        .execute_unprepared("UPDATE webdock_mail.send_job SET state='unknown' WHERE id='792'")
        .await
        .unwrap();
    for id in &grants[1..] {
        let tx = sending(worker).await;
        assert!(
            tx.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE webdock_mail.provider_lease SET attempted_at=now() WHERE id=$1::uuid",
                [id.clone().into()]
            ))
            .await
            .is_err()
        );
        tx.rollback().await.unwrap();
    }
    let tx = sending(worker).await;
    assert!(
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_mail.provider_lease SET revoked_at=NULL WHERE id=$1::uuid",
            [grants[1].clone().into()]
        ))
        .await
        .is_err()
    );
    tx.rollback().await.unwrap();
    let tx = sending(worker).await;
    assert!(
        tx.execute_unprepared("DELETE FROM webdock_mail.provider_lease")
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let unsigned = worker
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*)::bigint AS n FROM webdock_mail.provider_lease",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unsigned.try_get::<i64>("", "n").unwrap(), 0);
}
