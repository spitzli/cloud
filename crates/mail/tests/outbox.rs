//! Real restricted-role RLS fixture; unique tenant only, no deployment or delivery.
use hmac::{Hmac, Mac};
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait,
};
use serde_json::json;
use sha2::Sha256;
use webdock_mail::outbox::*;

async fn db(key: &str) -> DatabaseConnection {
    let url = std::env::var(key).expect("explicit owned local fixture");
    let parsed = url::Url::parse(&url).unwrap();
    assert!(
        matches!(parsed.host_str(), Some("localhost" | "127.0.0.1"))
            && parsed.path() == "/webdock_rls_test"
            && parsed.query().is_none()
    );
    Database::connect(url).await.unwrap()
}
async fn sql(db: &impl ConnectionTrait, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}
async fn tx(db: &DatabaseConnection, purpose: &str) -> DatabaseTransaction {
    let tx = db.begin().await.unwrap();
    let raw=json!({"v":1,"aud":"webdock_worker_runtime","exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"system","purpose":purpose}}).to_string();
    let mut mac =
        Hmac::<Sha256>::new_from_slice(std::env::var("DATABASE_RLS_SECRET").unwrap().as_bytes())
            .unwrap();
    mac.update(raw.as_bytes());
    sql(&tx,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",vec![raw.into(),hex::encode(mac.finalize().into_bytes()).into()]).await;
    tx
}
async fn next_id(db: &DatabaseConnection) -> String {
    db.query_one_raw(Statement::from_string(
        DbBackend::Postgres,
        "SELECT webdock_auth.next_snowflake() AS id",
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "id")
    .unwrap()
}

#[tokio::test]
#[ignore = "requires explicitly owned local RLS fixture"]
async fn durable_outbox_fences_reclaims_and_never_retries_unknown_handoffs() {
    let owner = db("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
    let worker = db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await;
    let customer = next_id(&owner).await;
    sql(
        &owner,
        "INSERT INTO webdock_admin.customers(id,name) VALUES($1,'Rust Mail outbox fixture')",
        vec![customer.clone().into()],
    )
    .await;
    let input = Enqueue {
        id: next_id(&owner).await,
        customer_id: customer.clone(),
        source_id: "key:fixture".into(),
        idempotency_key: "same-request".into(),
        request_hash: "a".repeat(64),
        payload_ref: uuid::Uuid::new_v4().to_string(),
        payload_bytes: 100,
        route: json!({"kind":"direct","revision":"1"}),
        envelope: json!({}),
        payload_format: PayloadFormat::Request,
        source_context: json!({}),
        staging: false,
        not_before: None,
    };
    let t = tx(&worker, "mail-send-gateway").await;
    let queued = enqueue(&t, &input).await.unwrap();
    t.commit().await.unwrap();
    let t = tx(&worker, "mail-send-gateway").await;
    let again = enqueue(&t, &input).await.unwrap();
    assert_eq!(again.id, queued.id);
    assert!(!again.created);
    t.commit().await.unwrap();
    let mut changed = input.clone();
    changed.request_hash = "b".repeat(64);
    let t = tx(&worker, "mail-send-gateway").await;
    assert!(enqueue(&t, &changed).await.is_err());
    t.rollback().await.unwrap();
    let wrong = tx(&worker, "plugin-runtime").await;
    assert!(claim(&wrong, Some(&customer)).await.unwrap().is_none());
    wrong.rollback().await.unwrap();
    let mut claims = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let worker = worker.clone();
        let customer = customer.clone();
        claims.spawn(async move {
            let t = tx(&worker, "mail-send-gateway").await;
            let result = claim(&t, Some(&customer)).await.unwrap();
            t.commit().await.unwrap();
            result
        });
    }
    let mut held = Vec::new();
    while let Some(result) = claims.join_next().await {
        if let Some(lease) = result.unwrap() {
            held.push(lease);
        }
    }
    assert_eq!(held.len(), 1);
    let first = held.pop().unwrap();
    sql(
        &owner,
        "UPDATE webdock_mail.send_job SET lease_until=now()-interval '1 second' WHERE id=$1",
        vec![queued.id.clone().into()],
    )
    .await;
    let t = tx(&worker, "mail-send-gateway").await;
    let second = claim(&t, Some(&customer)).await.unwrap().unwrap();
    assert_ne!(first.lease_token, second.lease_token);
    assert!(!begin_handoff(&t, &first).await.unwrap());
    assert!(begin_handoff(&t, &second).await.unwrap());
    assert!(!begin_handoff(&t, &second).await.unwrap());
    assert!(!cancel(&t, &customer, &queued.id).await.unwrap());
    t.commit().await.unwrap();
    sql(
        &owner,
        "UPDATE webdock_mail.send_job SET lease_until=now()-interval '1 second' WHERE id=$1",
        vec![queued.id.clone().into()],
    )
    .await;
    let t = tx(&worker, "mail-send-gateway").await;
    assert!(claim(&t, Some(&customer)).await.unwrap().is_none());
    let receipt = Receipt::Accepted {
        provider_id: "12".into(),
    };
    assert!(!finish_handoff(&t, &first, &receipt).await.unwrap());
    assert!(finish_handoff(&t, &second, &receipt).await.unwrap());
    assert!(finish_handoff(&t, &second, &receipt).await.unwrap());
    assert!(
        !finish_handoff(&t, &second, &Receipt::Rejected {})
            .await
            .unwrap()
    );
    t.commit().await.unwrap();
    let mut staged = input.clone();
    staged.id = next_id(&owner).await;
    staged.idempotency_key = "stage".into();
    staged.payload_ref = uuid::Uuid::new_v4().to_string();
    staged.staging = true;
    let t = tx(&worker, "mail-send-gateway").await;
    let saved = enqueue(&t, &staged).await.unwrap();
    assert!(claim(&t, Some(&customer)).await.unwrap().is_none());
    let payload = PayloadLease {
        id: saved.id,
        customer_id: customer.clone(),
        payload_ref: staged.payload_ref.clone(),
        upload_token: saved.upload_token.unwrap(),
    };
    let mut forged = payload.clone();
    forged.customer_id = "123".into();
    assert!(!commit_payload(&t, &forged).await.unwrap());
    assert!(commit_payload(&t, &payload).await.unwrap());
    let held = claim(&t, Some(&customer)).await.unwrap().unwrap();
    assert!(review_preparation(&t, &held).await.unwrap());
    assert!(!begin_handoff(&t, &held).await.unwrap());
    assert!(claim(&t, Some(&customer)).await.unwrap().is_none());
    assert!(cancel(&t, &customer, &staged.id).await.unwrap());
    t.commit().await.unwrap();
    sql(
        &owner,
        "DELETE FROM webdock_mail.send_job WHERE customer_id=$1",
        vec![customer.clone().into()],
    )
    .await;
    let organization:String=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1 RETURNING organization_id",[customer.clone().into()])).await.unwrap().unwrap().try_get("","organization_id").unwrap();
    sql(
        &owner,
        "DELETE FROM webdock_admin.customers WHERE id=$1",
        vec![customer.into()],
    )
    .await;
    sql(
        &owner,
        "DELETE FROM webdock_auth.organization WHERE id=$1",
        vec![organization.into()],
    )
    .await;
}
