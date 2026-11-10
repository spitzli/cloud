#[path = "support/native_pg.rs"]
mod fixture;
use spitzli_ids::{Identity, Layout};
use spitzli_persistence::IdAllocator;
use std::{collections::HashSet, sync::Arc};

#[tokio::test]
#[ignore = "owned local PostgreSQL18"]
async fn native_ids_are_durable_across_concurrency_restart_and_business_rollback() {
    let f = fixture::Fixture::start().await;
    let identity = Identity::platform(0, 2).unwrap();
    let allocator = Arc::new(
        IdAllocator::connect_native(&f.runtime_url, identity, "webdock_auth_runtime", false)
            .await
            .unwrap(),
    );
    let time = Layout::PLATFORM.epoch_ms() + 1_000_000;
    let first = allocator.allocate_at(time).await.unwrap();
    let mut tasks = Vec::new();
    let second = Arc::new(
        IdAllocator::connect_native(&f.runtime_url, identity, "webdock_auth_runtime", false)
            .await
            .unwrap(),
    );
    for index in 0..32 {
        let allocator = if index % 2 == 0 {
            allocator.clone()
        } else {
            second.clone()
        };
        tasks.push(tokio::spawn(async move {
            allocator.allocate_at(time).await.unwrap()
        }));
    }
    let mut ids = HashSet::from([first]);
    for task in tasks {
        assert!(ids.insert(task.await.unwrap()));
    }
    let restarted =
        IdAllocator::connect_native(&f.runtime_url, identity, "webdock_auth_runtime", false)
            .await
            .unwrap();
    let next = restarted.allocate_at(time).await.unwrap();
    assert!(!ids.contains(&next));
    let business = spitzli_persistence::postgres::Pool::connect(
        &f.runtime_url,
        "webdock_auth_runtime",
        false,
        Default::default(),
    )
    .await
    .unwrap();
    let mut connection = business.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let committed = restarted.allocate_at(time).await.unwrap();
    tx.rollback().await.unwrap();
    assert!(restarted.allocate_at(time).await.unwrap() > committed);
    assert!(restarted.allocate_at(time - 1).await.is_err());
    use sea_orm::{ConnectionTrait, DbBackend, Statement};
    let stored = || {
        Statement::from_string(
            DbBackend::Postgres,
            "SELECT high_water FROM webdock_ids.generator WHERE slot=2",
        )
    };
    let before: i64 = f
        .owner
        .query_one_raw(stored())
        .await
        .unwrap()
        .unwrap()
        .try_get("", "high_water")
        .unwrap();
    f.owner.execute_unprepared("CREATE FUNCTION webdock_ids.owned_commit_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'owned deferred commit failure'; END $$; CREATE CONSTRAINT TRIGGER owned_commit_failure AFTER UPDATE ON webdock_ids.generator DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION webdock_ids.owned_commit_failure()").await.unwrap();
    assert!(
        restarted.allocate_at(time).await.is_err(),
        "failed commit cannot release an ID"
    );
    let after: i64 = f
        .owner
        .query_one_raw(stored())
        .await
        .unwrap()
        .unwrap()
        .try_get("", "high_water")
        .unwrap();
    assert_eq!(before, after);
    f.owner.execute_unprepared("DROP TRIGGER owned_commit_failure ON webdock_ids.generator; DROP FUNCTION webdock_ids.owned_commit_failure()").await.unwrap();
    assert!(restarted.allocate_at(time).await.unwrap().as_i64() > before);
    f.owner
        .execute_unprepared("ALTER ROLE webdock_auth_runtime SUPERUSER")
        .await
        .unwrap();
    assert!(
        matches!(
            restarted.allocate_at(time).await,
            Err(spitzli_persistence::Error::Native(
                spitzli_persistence::postgres::Error::Authority
            ))
        ),
        "the fused generator-row query must enforce the same live-role predicate"
    );
    f.owner
        .execute_unprepared("ALTER ROLE webdock_auth_runtime NOSUPERUSER")
        .await
        .unwrap();
    assert!(restarted.allocate_at(time).await.unwrap().as_i64() > before);
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; non-gating latency experiment"]
async fn compare_allocator_driver_latency_without_claiming_a_speedup() {
    let f = fixture::Fixture::start().await;
    let identity = Identity::platform(0, 2).unwrap();
    let native =
        IdAllocator::connect_native(&f.runtime_url, identity, "webdock_auth_runtime", false)
            .await
            .unwrap();
    let legacy = IdAllocator::connect(&f.runtime_url, identity)
        .await
        .unwrap();
    for allocator in [&legacy, &native] {
        for _ in 0..20 {
            allocator.allocate().await.unwrap();
        }
    }
    let mut legacy_samples = Vec::new();
    let mut native_samples = Vec::new();
    for round in 0..4 {
        for offset in 0..2 {
            let native_turn = (round + offset) % 2 == 1;
            let (allocator, samples) = if native_turn {
                (&native, &mut native_samples)
            } else {
                (&legacy, &mut legacy_samples)
            };
            for _ in 0..50 {
                let start = std::time::Instant::now();
                allocator.allocate().await.unwrap();
                samples.push(start.elapsed().as_micros());
            }
        }
    }
    for (name, samples) in [
        ("legacy", &mut legacy_samples),
        ("native", &mut native_samples),
    ] {
        samples.sort_unstable();
        println!(
            "allocator_benchmark driver={name} samples={} p50_us={} p95_us={} p99_us={}",
            samples.len(),
            samples[samples.len() / 2],
            samples[samples.len() * 95 / 100],
            samples[samples.len() * 99 / 100]
        );
    }
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18 with a commit-response-loss proxy"]
async fn lost_commit_ack_never_releases_or_reuses_an_id() {
    use sea_orm::{ConnectionTrait, DbBackend, Statement};
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = fixture::Fixture::start().await;
    let mut url = url::Url::parse(&f.runtime_url).unwrap();
    let backend_port = url.port().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    url.set_port(Some(listener.local_addr().unwrap().port()))
        .unwrap();
    let drop_once = Arc::new(AtomicBool::new(true));
    let dropped = drop_once.clone();
    let proxy = tokio::spawn(async move {
        loop {
            let (client, _) = listener.accept().await.unwrap();
            tokio::spawn(proxy_commit_connection(
                client,
                backend_port,
                drop_once.clone(),
            ));
        }
    });
    let allocator = IdAllocator::connect_native(
        url.as_str(),
        Identity::platform(0, 2).unwrap(),
        "webdock_auth_runtime",
        false,
    )
    .await
    .unwrap();
    let time = Layout::PLATFORM.epoch_ms() + 1_000_000;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(33),
        allocator.allocate_at(time),
    )
    .await;
    assert!(
        result.is_ok(),
        "native allocator must bound a silently withheld commit response"
    );
    assert!(
        result.unwrap().is_err(),
        "unconfirmed commit must not return an ID"
    );
    assert!(!dropped.load(Ordering::SeqCst));
    let lost: i64 = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT high_water FROM webdock_ids.generator WHERE slot=2",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "high_water")
        .unwrap();
    assert!(
        allocator.allocate_at(time).await.unwrap().as_i64() > lost,
        "retry must advance past the durably committed but unconfirmed allocation"
    );
    proxy.abort();
}

async fn proxy_commit_connection(
    client: tokio::net::TcpStream,
    port: u16,
    drop_once: Arc<std::sync::atomic::AtomicBool>,
) {
    let server = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let (mut cr, cw) = client.into_split();
    let (sr, mut sw) = server.into_split();
    tokio::select! {_=tokio::io::copy(&mut cr,&mut sw)=>{},_=forward_commit_responses(sr,cw,drop_once)=>{}}
}
async fn forward_commit_responses(
    mut sr: tokio::net::tcp::OwnedReadHalf,
    mut cw: tokio::net::tcp::OwnedWriteHalf,
    drop_once: Arc<std::sync::atomic::AtomicBool>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    loop {
        let mut header = [0u8; 5];
        if sr.read_exact(&mut header).await.is_err() {
            return;
        }
        let length = u32::from_be_bytes(header[1..5].try_into().unwrap()) as usize;
        assert!((4..=1024 * 1024).contains(&length));
        let mut body = vec![0u8; length - 4];
        if sr.read_exact(&mut body).await.is_err() {
            return;
        }
        if header[0] == b'C'
            && body == b"COMMIT\0"
            && drop_once.swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            // Keep both sockets open: server statement_timeout cannot bound a lost response.
            std::future::pending::<()>().await;
            return;
        }
        if cw.write_all(&header).await.is_err() || cw.write_all(&body).await.is_err() {
            return;
        }
    }
}
