use sea_orm::{ConnectionTrait, Database, DbBackend, Statement, TransactionTrait};
use std::{collections::HashSet, sync::Arc};
use webdock_ids::{Identity, Layout};
use webdock_persistence::IdAllocator;

#[tokio::test]
#[ignore = "requires explicitly owned local WEBDOCK_ID_TEST_DATABASE_URL"]
async fn allocated_ids_survive_restart_failed_business_transactions_and_concurrent_instances() {
    let raw = std::env::var("WEBDOCK_ID_TEST_DATABASE_URL").unwrap();
    let url = url::Url::parse(&raw).unwrap();
    assert!(
        matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
            && url.path().ends_with("_test")
            && url.query().is_none()
    );
    let db = Database::connect(&raw).await.unwrap();
    db.execute_unprepared(include_str!("../migrations/0001_ids.sql"))
        .await
        .unwrap();
    let identity = Identity::platform(0, 2).unwrap();
    let allocator = Arc::new(IdAllocator::new(db.clone(), identity));
    let time = Layout::PLATFORM.epoch_ms() + 1_000_000;
    let first = allocator.allocate_at(time).await.unwrap();
    let mut tasks = vec![];
    for _ in 0..32 {
        let alloc = allocator.clone();
        tasks.push(tokio::spawn(async move {
            alloc.allocate_at(time).await.unwrap()
        }));
    }
    let mut ids = HashSet::from([first]);
    for task in tasks {
        assert!(ids.insert(task.await.unwrap()));
    }
    let restarted = IdAllocator::new(db.clone(), identity);
    let next = restarted.allocate_at(time).await.unwrap();
    assert!(!ids.contains(&next));
    let business = db.begin().await.unwrap();
    let committed_id = restarted.allocate_at(time).await.unwrap();
    business.rollback().await.unwrap();
    let next = restarted.allocate_at(time).await.unwrap();
    assert!(
        next > committed_id,
        "a business rollback must not rewind ID allocation"
    );
    assert!(restarted.allocate_at(time - 1).await.is_err());
    let stored = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT high_water FROM webdock_ids.generator WHERE slot=2".to_owned(),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "high_water")
        .unwrap();
    assert_eq!(stored, next.as_i64());
    // Runtime cannot move a high-water mark backwards or change the layout/identity.
    assert!(
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_ids.generator SET high_water=$1 WHERE slot=2",
            [first.as_i64().into()]
        ))
        .await
        .is_err()
    );
}
