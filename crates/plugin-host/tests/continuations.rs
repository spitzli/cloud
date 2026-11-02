use futures_util::FutureExt;
use hmac::{Hmac, Mac};
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait,
};
use serde_json::json;
use sha2::Sha256;
const KEY: &str = "plugin-continuation-owned-fixture-signing-secret";

async fn scope(worker: &DatabaseConnection, purpose: Option<&str>) -> DatabaseTransaction {
    let tx = worker.begin().await.unwrap();
    if let Some(purpose) = purpose {
        let raw=json!({"v":1,"aud":"webdock_worker_runtime","exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"system","purpose":purpose,"authProvider":"better-auth"}}).to_string();
        let mut mac = Hmac::<Sha256>::new_from_slice(KEY.as_bytes()).unwrap();
        mac.update(raw.as_bytes());
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",[raw.into(),hex::encode(mac.finalize().into_bytes()).into()])).await.unwrap();
    }
    tx
}
async fn result(worker: &DatabaseConnection, purpose: Option<&str>, claim: &str) -> Option<bool> {
    let tx = scope(worker, purpose).await;
    let value = tx
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.plugin_state_continuations('789',$1::uuid) AS active",
            [claim.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "active")
        .unwrap();
    tx.commit().await.unwrap();
    value
}
#[tokio::test]
#[ignore = "creates and removes one owned local database for continuation-fence migration"]
async fn retained_jobs_authorized_scope_and_installation_lock_fence_disable() {
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
        "webdock_plugin_continuation_{}",
        uuid::Uuid::new_v4().simple()
    );
    admin
        .execute_unprepared(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    owner_url.set_path(&format!("/{name}"));
    let outcome=std::panic::AssertUnwindSafe(async {
        let owner=Database::connect(owner_url.as_str()).await.unwrap();
        webdock_persistence::migrations::bootstrap(&owner,&name,KEY).await.unwrap();
        let seed=owner.begin().await.unwrap();
        seed.execute_unprepared("SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"123\",\"organization_id\":\"124\"}',true)").await.unwrap();
        seed.execute_unprepared("INSERT INTO webdock_admin.customers(id,name) VALUES('123','Continuation fixture')").await.unwrap();
        seed.commit().await.unwrap();
        owner.execute_unprepared("INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,generation) VALUES('provider-fixture','external','draining','disabled','1.0.0',7),('other-provider','external','enabled','enabled','1.0.0',1)").await.unwrap();
        let claim=uuid::Uuid::new_v4().to_string();
        owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_operation(id,plugin_id,actor_id,idempotency_key,payload,status,generation,claim_token,lease_until) VALUES('789','provider-fixture','fixture','fixture','{\"desired\":\"disabled\"}','running',7,$1::uuid,now()+interval '5 minutes')",[claim.clone().into()])).await.unwrap();
        owner.execute_unprepared("INSERT INTO webdock_mail.provider_binding(customer_id,plugin_id,revision,transport_revision) VALUES('123','provider-fixture',1,1)").await.unwrap();
        let mut worker_url=url::Url::parse(&std::env::var("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").unwrap()).unwrap();
        assert_eq!(worker_url.host_str(),owner_url.host_str());assert_eq!(worker_url.port(),owner_url.port());worker_url.set_path(owner_url.path());
        let worker=Database::connect(worker_url.as_str()).await.unwrap();
        for purpose in [None,Some("mail-send-gateway"),Some("plugin-admission")] {
            assert_eq!(result(&worker,purpose,&claim).await,None);
        }
        let purpose=Some("plugin-controller");
        assert_eq!(result(&worker,purpose,&uuid::Uuid::new_v4().to_string()).await,None);
        assert_eq!(result(&worker,purpose,&claim).await,Some(false));
        owner.execute_unprepared("UPDATE webdock_auth.plugin_operation SET lease_until=now()-interval '1 second' WHERE id='789'").await.unwrap();
        assert_eq!(result(&worker,purpose,&claim).await,None);
        owner.execute_unprepared("UPDATE webdock_auth.plugin_operation SET lease_until=now()+interval '5 minutes' WHERE id='789'; UPDATE webdock_auth.plugin_installation SET generation=8 WHERE id='provider-fixture'").await.unwrap();
        assert_eq!(result(&worker,purpose,&claim).await,None);
        owner.execute_unprepared("UPDATE webdock_auth.plugin_installation SET generation=7 WHERE id='provider-fixture'").await.unwrap();
        owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_mail.send_job(id,customer_id,source_id,idempotency_key,request_hash,payload_ref,payload_bytes,route,state,created_at) VALUES('890','123','fixture','fixture',$1,$2::uuid,100,'{\"kind\":\"direct\",\"pluginID\":\"provider-fixture\"}','queued',now()-interval '2 hours')",["a".repeat(64).into(),uuid::Uuid::new_v4().to_string().into()])).await.unwrap();
        for state in ["staging","queued","leased","preparation_review","handoff","unknown"] {
            owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_mail.send_job SET state=$1 WHERE id='890'",[state.into()])).await.unwrap();
            assert_eq!(result(&worker,purpose,&claim).await,Some(true),"{state}");
        }
        for state in ["accepted","rejected","cancelled"] {
            owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_mail.send_job SET state=$1 WHERE id='890'",[state.into()])).await.unwrap();
            assert_eq!(result(&worker,purpose,&claim).await,Some(false),"{state}");
        }
        owner.execute_unprepared("UPDATE webdock_mail.send_job SET state='queued',route='{\"kind\":\"direct\",\"pluginID\":\"other-provider\"}' WHERE id='890'").await.unwrap();
        assert_eq!(result(&worker,purpose,&claim).await,Some(false),"another native plugin must never be attributed to the current binding");
        owner.execute_unprepared("UPDATE webdock_mail.send_job SET route='{\"kind\":\"direct\"}' WHERE id='890'").await.unwrap();
        assert_eq!(result(&worker,purpose,&claim).await,Some(true),"legacy pending work conservatively retains the bound provider");
        owner.execute_unprepared("UPDATE webdock_mail.send_job SET state='accepted' WHERE id='890'").await.unwrap();
        let tx=owner.begin().await.unwrap();
        tx.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT id FROM webdock_auth.plugin_installation WHERE id='provider-fixture' FOR SHARE")).await.unwrap();
        let pending_worker=worker.clone();let pending_claim=claim.clone();
        let pending=tokio::spawn(async move {result(&pending_worker,Some("plugin-controller"),&pending_claim).await});
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!pending.is_finished(),"disable must wait for the domain installation fence");
        tx.execute_unprepared("UPDATE webdock_mail.send_job SET state='queued' WHERE id='890'").await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(pending.await.unwrap(),Some(true),"the post-lock snapshot must see committed accepted work");
        worker.close().await.unwrap();owner.close().await.unwrap();
    }).catch_unwind().await;
    admin
        .execute_unprepared(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    if let Err(error) = outcome {
        std::panic::resume_unwind(error);
    }
}
