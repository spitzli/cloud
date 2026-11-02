//! The plugin's own database: one row per app. The Core never reads it.
use tokio_postgres::Client;
use uuid::Uuid;

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE apps (
        id uuid PRIMARY KEY,
        project_id text NOT NULL,
        name text NOT NULL,
        image text NOT NULL,
        port integer NOT NULL,
        memory_mb integer NOT NULL,
        env_sealed bytea NOT NULL,
        running boolean NOT NULL DEFAULT true,
        state text NOT NULL CHECK (state IN ('deploying','ready','failed')),
        error text,
        created_at timestamptz NOT NULL DEFAULT now()
    )",
    // Apps run on the cluster; their state comes from the agent's observation.
    "ALTER TABLE apps DROP COLUMN state, DROP COLUMN error, ADD COLUMN restart text NOT NULL DEFAULT ''",
    // The customer's own domains the app answers on besides its own host.
    "ALTER TABLE apps ADD COLUMN domains text[] NOT NULL DEFAULT '{}'",
];

pub type Error = tokio_postgres::Error;
#[derive(Clone, Debug)]
pub struct App {
    pub id: Uuid,
    pub project_id: String,
    pub name: String,
    pub image: String,
    pub port: i32,
    pub memory_mb: i32,
    pub env_sealed: Vec<u8>,
    pub running: bool,
    /// A new value rolls the app's pods.
    pub restart: String,
    pub domains: Vec<String>,
    pub created_at: String,
}
const COLUMNS: &str =
    "id,project_id,name,image,port,memory_mb,env_sealed,running,restart,domains,created_at::text";
fn app(r: &tokio_postgres::Row) -> App {
    App {
        id: r.get(0),
        project_id: r.get(1),
        name: r.get(2),
        image: r.get(3),
        port: r.get(4),
        memory_mb: r.get(5),
        env_sealed: r.get(6),
        running: r.get(7),
        restart: r.get(8),
        domains: r.get(9),
        created_at: r.get(10),
    }
}

pub struct Store(Client);
impl Store {
    pub async fn connect(url: &str) -> Result<Self, Error> {
        Ok(Self(
            webdock_plugin_kit::storage(url, "hosting", MIGRATIONS).await?,
        ))
    }
    pub async fn insert(&self, a: &App) -> Result<(), Error> {
        self.0.execute("INSERT INTO apps(id,project_id,name,image,port,memory_mb,env_sealed,running,restart,domains) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            &[&a.id, &a.project_id, &a.name, &a.image, &a.port, &a.memory_mb, &a.env_sealed, &a.running, &a.restart, &a.domains]).await?;
        Ok(())
    }
    /// An app of this project only; another project's id reads as missing.
    pub async fn get(&self, project: &str, id: &Uuid) -> Result<Option<App>, Error> {
        Ok(self
            .0
            .query_opt(
                &format!("SELECT {COLUMNS} FROM apps WHERE id=$1 AND project_id=$2"),
                &[id, &project],
            )
            .await?
            .as_ref()
            .map(app))
    }
    pub async fn update(&self, a: &App) -> Result<(), Error> {
        self.0.execute("UPDATE apps SET image=$2,port=$3,memory_mb=$4,env_sealed=$5,running=$6,restart=$7,domains=$8 WHERE id=$1",
            &[&a.id, &a.image, &a.port, &a.memory_mb, &a.env_sealed, &a.running, &a.restart, &a.domains]).await?;
        Ok(())
    }
    pub async fn delete(&self, id: &Uuid) -> Result<(), Error> {
        self.0
            .execute("DELETE FROM apps WHERE id=$1", &[id])
            .await?;
        Ok(())
    }
}
