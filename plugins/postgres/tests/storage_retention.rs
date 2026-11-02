//! Physical Store retention proof; signed release/Console lifecycle is a separate acceptance.
use std::{process::Command, time::Duration};
use tokio_postgres::{Client, NoTls};
use uuid::Uuid;
use webdock_postgres::{
    seal::Sealer,
    store::{Resource, Store},
};

fn docker() -> Command {
    let mut command = Command::new("docker");
    command.args(["--host", "unix:///var/run/docker.sock"]);
    command
}

struct Fixture {
    container: String,
    url: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = docker()
            .args(["rm", "--force", "--volumes", &self.container])
            .output();
    }
}
impl Fixture {
    async fn start() -> Self {
        assert_eq!(
            std::env::var("WEBDOCK_POSTGRES_STORAGE_DOCKER_TEST").as_deref(),
            Ok("1")
        );
        let container = format!("webdock-postgres-storage-{}", Uuid::new_v4().simple());
        let password = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let started = docker()
            .args([
                "run",
                "--detach",
                "--name",
                &container,
                "--label",
                "webdock.owned-postgres-storage=true",
                "--env",
                "POSTGRES_PASSWORD",
                "--publish",
                "127.0.0.1::5432",
                "postgres:17-alpine",
            ])
            .env("POSTGRES_PASSWORD", &password)
            .output()
            .unwrap();
        assert!(
            started.status.success(),
            "owned PostgreSQL startup failed; diagnostics withheld"
        );
        // Install cleanup custody before looking up the published port or waiting for readiness.
        let mut fixture = Self {
            container,
            url: String::new(),
        };
        let output = docker()
            .args(["port", &fixture.container, "5432/tcp"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "owned PostgreSQL port lookup failed"
        );
        let address = String::from_utf8(output.stdout).unwrap();
        let port: u16 = address
            .trim()
            .strip_prefix("127.0.0.1:")
            .unwrap()
            .parse()
            .unwrap();
        let mut url = url::Url::parse("postgresql://postgres@127.0.0.1/postgres").unwrap();
        url.set_port(Some(port)).unwrap();
        url.set_password(Some(&password)).unwrap();
        fixture.url = url.into();
        for attempt in 0..100 {
            if fixture.connect().await.is_ok() {
                return fixture;
            }
            assert!(
                attempt < 99,
                "owned PostgreSQL readiness failed; diagnostics withheld"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        unreachable!()
    }
    async fn connect(&self) -> Result<Client, tokio_postgres::Error> {
        let (client, connection) = tokio_postgres::connect(&self.url, NoTls).await?;
        tokio::spawn(connection);
        Ok(client)
    }
}

#[tokio::test]
#[ignore = "requires WEBDOCK_POSTGRES_STORAGE_DOCKER_TEST=1; creates only an owned disposable PostgreSQL"]
async fn physical_three_migrations_and_sealed_resource_survive_repeated_store_startup() {
    let fixture = Fixture::start().await;
    let admin = fixture.connect().await.unwrap();
    let key = "61".repeat(32);
    let sealer = Sealer::new(&key).unwrap();
    let id = Uuid::new_v4();
    let password = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let sealed = sealer.seal(&id, &password);
    let store = Store::connect(&fixture.url).await.unwrap();
    store
        .insert(&Resource {
            id,
            project_id: "owned-project".into(),
            offering: "dedicated".into(),
            name: "Retained resource".into(),
            server_id: None,
            database_name: "owned_database".into(),
            role_name: "owned_role".into(),
            password_sealed: sealed.clone(),
            host: Some("database.test".into()),
            port: Some(5432),
            version: Some("17".into()),
            state: "ready".into(),
            error: None,
            created_at: String::new(),
        })
        .await
        .unwrap();
    drop(store);
    let original: String = admin
        .query_one(
            "SELECT to_jsonb(resources)::text FROM resources WHERE id=$1",
            &[&id],
        )
        .await
        .unwrap()
        .get(0);
    admin.batch_execute("CREATE TABLE owned_sql_sentinel(value integer PRIMARY KEY); INSERT INTO owned_sql_sentinel VALUES(73)").await.unwrap();

    for _ in 0..3 {
        let store = Store::connect(&fixture.url).await.unwrap();
        let versions: Vec<i32> = admin
            .query(
                "SELECT version FROM schema_migrations ORDER BY version",
                &[],
            )
            .await
            .unwrap()
            .iter()
            .map(|row| row.get(0))
            .collect();
        assert_eq!(
            versions,
            [1, 2, 3],
            "private migration history is unchanged on restart"
        );
        assert!(!admin.query_one("SELECT EXISTS(SELECT FROM information_schema.columns WHERE table_schema='public' AND table_name='resources' AND column_name='pending_sealed')", &[]).await.unwrap().get::<_, bool>(0));
        let retained = store.get("owned-project", &id).await.unwrap().unwrap();
        assert!(
            retained.password_sealed == sealed,
            "startup never reseals or replaces stored credentials"
        );
        let reopened = Sealer::new(&key)
            .unwrap()
            .open(&id, &retained.password_sealed);
        assert!(
            reopened.as_deref() == Some(password.as_str()),
            "original key still decrypts the original password"
        );
        assert!(
            Sealer::new(&"62".repeat(32))
                .unwrap()
                .open(&id, &retained.password_sealed)
                .is_none()
        );
        assert!(store.get("other-project", &id).await.unwrap().is_none());
        let row: String = admin
            .query_one(
                "SELECT to_jsonb(resources)::text FROM resources WHERE id=$1",
                &[&id],
            )
            .await
            .unwrap()
            .get(0);
        assert!(
            row == original,
            "every persisted resource field remains unchanged"
        );
        assert_eq!(
            admin
                .query_one("SELECT value FROM owned_sql_sentinel", &[])
                .await
                .unwrap()
                .get::<_, i32>(0),
            73
        );
    }
}
