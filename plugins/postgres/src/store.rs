//! The plugin's own database: one row per customer database or instance. The Core never reads it.
use tokio_postgres::Client;
use uuid::Uuid;

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE resources (
        id uuid PRIMARY KEY,
        project_id text NOT NULL,
        offering text NOT NULL CHECK (offering IN ('shared','dedicated')),
        name text NOT NULL,
        server_id text,
        database_name text NOT NULL,
        role_name text NOT NULL,
        password_sealed bytea NOT NULL,
        host text,
        port integer,
        version text,
        state text NOT NULL CHECK (state IN ('creating','ready','failed')),
        error text,
        created_at timestamptz NOT NULL DEFAULT now(),
        updated_at timestamptz NOT NULL DEFAULT now()
    );
    CREATE INDEX resources_server ON resources(server_id) WHERE server_id IS NOT NULL;",
    "ALTER TABLE resources ADD COLUMN pending_sealed bytea",
    // Dedicated rotations are declarative now; nothing is staged.
    "ALTER TABLE resources DROP COLUMN pending_sealed",
];

#[derive(Clone, Debug)]
pub struct Resource {
    pub id: Uuid,
    pub project_id: String,
    pub offering: String,
    pub name: String,
    pub server_id: Option<String>,
    pub database_name: String,
    pub role_name: String,
    pub password_sealed: Vec<u8>,
    pub host: Option<String>,
    pub port: Option<i32>,
    pub version: Option<String>,
    pub state: String,
    pub error: Option<String>,
    pub created_at: String,
}
const COLUMNS: &str = "id,project_id,offering,name,server_id,database_name,role_name,password_sealed,host,port,version,state,error,created_at::text";
fn resource(row: &tokio_postgres::Row) -> Resource {
    Resource {
        id: row.get(0),
        project_id: row.get(1),
        offering: row.get(2),
        name: row.get(3),
        server_id: row.get(4),
        database_name: row.get(5),
        role_name: row.get(6),
        password_sealed: row.get(7),
        host: row.get(8),
        port: row.get(9),
        version: row.get(10),
        state: row.get(11),
        error: row.get(12),
        created_at: row.get(13),
    }
}

pub type Error = webdock_plugin_kit::Error;
pub struct Store(Client);
impl Store {
    /// Connects and applies pending migrations under an advisory lock.
    pub async fn connect(url: &str) -> Result<Self, Error> {
        webdock_plugin_kit::loopback(url)?;
        Ok(Self(
            webdock_plugin_kit::storage(url, "postgres", MIGRATIONS).await?,
        ))
    }
    /// Remote metadata storage verifies the actual PostgreSQL hostname with the private CA.
    pub async fn connect_tls(url: &str, ca: &[u8]) -> Result<Self, Error> {
        Ok(Self(
            webdock_plugin_kit::storage_tls(url, ca, "postgres", MIGRATIONS).await?,
        ))
    }
    pub async fn insert(&self, r: &Resource) -> Result<(), Error> {
        self.0.execute("INSERT INTO resources(id,project_id,offering,name,server_id,database_name,role_name,password_sealed,host,port,version,state) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
            &[&r.id,&r.project_id,&r.offering,&r.name,&r.server_id,&r.database_name,&r.role_name,&r.password_sealed,&r.host,&r.port,&r.version,&r.state]).await?;
        Ok(())
    }
    /// A resource of this project only; another project's id reads as missing.
    pub async fn get(&self, project: &str, id: &Uuid) -> Result<Option<Resource>, Error> {
        let row = self
            .0
            .query_opt(
                &format!("SELECT {COLUMNS} FROM resources WHERE id=$1 AND project_id=$2"),
                &[id, &project],
            )
            .await?;
        Ok(row.as_ref().map(resource))
    }
    pub async fn creating(&self) -> Result<Vec<Resource>, Error> {
        let rows = self
            .0
            .query(
                &format!("SELECT {COLUMNS} FROM resources WHERE state='creating'"),
                &[],
            )
            .await?;
        Ok(rows.iter().map(resource).collect())
    }
    pub async fn on_server(&self, server: &str) -> Result<i64, Error> {
        Ok(self
            .0
            .query_one(
                "SELECT count(*) FROM resources WHERE server_id=$1",
                &[&server],
            )
            .await?
            .get(0))
    }
    pub async fn ready(&self, id: &Uuid, host: &str, port: i32) -> Result<(), Error> {
        self.0.execute("UPDATE resources SET state='ready',host=$2,port=$3,error=NULL,updated_at=now() WHERE id=$1", &[id, &host, &port]).await?;
        Ok(())
    }
    pub async fn failed(&self, id: &Uuid, error: &str) -> Result<(), Error> {
        self.0
            .execute(
                "UPDATE resources SET state='failed',error=$2,updated_at=now() WHERE id=$1",
                &[id, &error],
            )
            .await?;
        Ok(())
    }
    pub async fn password(&self, id: &Uuid, sealed: &[u8]) -> Result<(), Error> {
        self.0
            .execute(
                "UPDATE resources SET password_sealed=$2,updated_at=now() WHERE id=$1",
                &[id, &sealed],
            )
            .await?;
        Ok(())
    }
    pub async fn delete(&self, id: &Uuid) -> Result<(), Error> {
        self.0
            .execute("DELETE FROM resources WHERE id=$1", &[id])
            .await?;
        Ok(())
    }
}
