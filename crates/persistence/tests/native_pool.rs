use std::time::Duration;
use webdock_persistence::postgres::{Error, Limits, Pool};
#[path = "support/native_pg.rs"]
mod fixture;

#[tokio::test]
async fn native_pool_rejects_ambiguous_or_unsafe_configuration_before_connecting() {
    for raw in [
        "https://webdock_auth_runtime:private@localhost/db",
        "postgres://webdock_auth_runtime:private@localhost/db?host=elsewhere.invalid",
        "postgres://webdock_auth_runtime:private@db.invalid/db?sslmode=require",
        "postgres://webdock_auth_runtime:private@localhost/db?sslmode=verify-full&ssl-mode=disable",
        "postgres://webdock_auth_runtime:private@localhost/db?options=-c%20role=postgres",
        "postgres://postgres:private@localhost/db",
    ] {
        let error = Pool::connect(raw, "webdock_auth_runtime", false, Limits::default())
            .await
            .err()
            .unwrap();
        assert!(matches!(error, Error::Configuration));
        assert!(!format!("{error:?} {error}").contains("private"));
    }
    let limits = Limits {
        max_connections: 0,
        ..Limits::default()
    };
    assert!(matches!(
        Pool::connect(
            "postgres://webdock_auth_runtime:private@localhost/db",
            "webdock_auth_runtime",
            false,
            limits
        )
        .await
        .err()
        .unwrap(),
        Error::Configuration
    ));
    let limits = Limits {
        wait_timeout: Duration::ZERO,
        ..Limits::default()
    };
    assert!(
        Pool::connect(
            "postgres://webdock_auth_runtime:private@localhost/db",
            "webdock_auth_runtime",
            false,
            limits
        )
        .await
        .is_err()
    );
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18"]
async fn native_pool_bounds_reuse_and_session_authority() {
    use sea_orm::ConnectionTrait;
    let fixture = fixture::Fixture::start().await;
    assert!(
        Pool::connect(&fixture.owner_url, "postgres", false, Limits::default())
            .await
            .is_err()
    );
    let pool = Pool::connect(
        &fixture.runtime_url,
        "webdock_auth_runtime",
        false,
        Limits {
            max_connections: 1,
            wait_timeout: Duration::from_millis(100),
            ..Limits::default()
        },
    )
    .await
    .unwrap();
    let mut connection = pool.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let pid: i32 = tx
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let budget = tx
        .query_one(
            "SELECT current_setting('statement_timeout'),current_setting('lock_timeout')",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(budget.get::<_, String>(0), "30s");
    assert_eq!(budget.get::<_, String>(1), "5s");
    assert!(
        pool.acquire().await.is_err(),
        "pool maximum must not grow under pressure"
    );
    tx.rollback().await.unwrap();
    drop(connection);
    let mut connection = pool.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    assert_eq!(
        tx.query_one("SELECT pg_backend_pid()", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        pid
    );
    tx.query_one(
        "SELECT set_config('webdock.rls_context','stale-tenant',false)",
        &[],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    drop(connection);
    let mut dirty = pool.acquire().await.unwrap();
    assert!(matches!(
        dirty.transaction().await.err().unwrap(),
        Error::Authority
    ));
    drop(dirty);
    let mut clean = pool.acquire().await.unwrap();
    let tx = clean.transaction().await.unwrap();
    assert_ne!(
        tx.query_one("SELECT pg_backend_pid()", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        pid
    );
    tx.rollback().await.unwrap();
    drop(clean);
    fixture
        .owner
        .execute_unprepared("ALTER ROLE webdock_auth_runtime SUPERUSER")
        .await
        .unwrap();
    let mut invalid = pool.acquire().await.unwrap();
    assert!(matches!(
        invalid.transaction().await.err().unwrap(),
        Error::Authority
    ));
    drop(invalid);
    fixture
        .owner
        .execute_unprepared("ALTER ROLE webdock_auth_runtime NOSUPERUSER")
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18"]
async fn native_pool_discards_cancelled_transactions_and_has_no_idle_queries() {
    let fixture = fixture::Fixture::start().await;
    let pool = Pool::connect(
        &fixture.runtime_url,
        "webdock_auth_runtime",
        false,
        Limits {
            max_connections: 1,
            min_connections: 0,
            idle_timeout: Duration::from_millis(100),
            ..Limits::default()
        },
    )
    .await
    .unwrap();
    let (entered, received) = tokio::sync::oneshot::channel();
    let cloned = pool.clone();
    let task = tokio::spawn(async move {
        let mut c = cloned.acquire().await.unwrap();
        let tx = c.transaction().await.unwrap();
        let pid = tx
            .query_one("SELECT pg_backend_pid()", &[])
            .await
            .unwrap()
            .get::<_, i32>(0);
        entered.send(pid).unwrap();
        tx.query_one("SELECT pg_sleep(10)", &[]).await.unwrap();
        tx.commit().await.unwrap();
    });
    let old_pid = received.await.unwrap();
    tokio::task::yield_now().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let mut c = tokio::time::timeout(Duration::from_secs(2), pool.acquire())
        .await
        .unwrap()
        .unwrap();
    let tx = tokio::time::timeout(Duration::from_secs(2), c.transaction())
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        tx.query_one("SELECT pg_backend_pid()", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        old_pid
    );
    tx.commit().await.unwrap();
    drop(c);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while pool.status().size != 0 {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}
