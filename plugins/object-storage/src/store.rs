//! Private installation PostgreSQL metadata. No filesystem storage or pooled lock sessions.
use crate::model::{ProviderError, ProviderResult, Resource};
use serde_json::Value;
use tokio_postgres::Client;
use uuid::Uuid;
pub type Result<T> = std::result::Result<T, &'static str>;
const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_metadata.sql")];
// Stable across runtime generations; every operation in this private database shares the fence.
const FENCE: i64 = 704611025;

pub struct Store {
    url: String,
    ca: Option<Vec<u8>>,
}
/// A dedicated session: dropping/cancelling it closes the connection and releases its lock.
/// Intent and completion transactions commit independently while this session lock stays held.
pub struct Fence {
    connection: Client,
}
impl Store {
    pub async fn connect(url: &str, ca: Option<&[u8]>) -> Result<Self> {
        let client = match ca {
            Some(ca) => webdock_plugin_kit::storage_tls(url, ca, "object-storage", MIGRATIONS)
                .await
                .map_err(|_| "metadata migration unavailable")?,
            None => {
                webdock_plugin_kit::loopback(url)
                    .map_err(|_| "plaintext metadata requires loopback")?;
                webdock_plugin_kit::storage(url, "object-storage", MIGRATIONS)
                    .await
                    .map_err(|_| "metadata migration unavailable")?
            }
        };
        let version: i32 = client
            .query_one(
                "SELECT COALESCE(max(version),0) FROM schema_migrations",
                &[],
            )
            .await
            .map_err(|_| "metadata version unavailable")?
            .get(0);
        if usize::try_from(version).ok() != Some(MIGRATIONS.len()) {
            return Err("metadata schema is newer than this runtime");
        }
        Ok(Self {
            url: url.into(),
            ca: ca.map(Vec::from),
        })
    }
    /// Independent read committed snapshot; live transfer checks cannot wait on the mutation fence.
    pub async fn read(&self, id: &Uuid) -> Result<Option<Resource>> {
        let connection = webdock_plugin_kit::postgres::connect(&self.url, self.ca.as_deref())
            .await
            .map_err(|_| "metadata unavailable")?;
        connection
            .batch_execute("SET statement_timeout='500ms'")
            .await
            .map_err(|_| "metadata unavailable")?;
        let row = connection
            .query_opt("SELECT body FROM resources WHERE id=$1", &[id])
            .await
            .map_err(|_| "metadata read failed")?;
        row.map(|r| serde_json::from_str(r.get::<_, &str>(0)).map_err(|_| "metadata damaged"))
            .transpose()
    }
    pub async fn lock(&self) -> Result<Fence> {
        let connection = webdock_plugin_kit::postgres::connect(&self.url, self.ca.as_deref())
            .await
            .map_err(|_| "metadata unavailable")?;
        // Do not set an idle-session timeout: external effects may be in progress while SQL is idle.
        connection
            .batch_execute("SET statement_timeout='5s'; SET synchronous_commit=on")
            .await
            .map_err(|_| "metadata unavailable")?;
        let locked: bool = connection
            .query_one("SELECT pg_try_advisory_lock($1)", &[&FENCE])
            .await
            .map_err(|_| "metadata fence unavailable")?
            .get(0);
        if !locked {
            return Err("another storage operation holds the fence");
        }
        Ok(Fence { connection })
    }
}
impl Fence {
    pub async fn check(&self) -> Result<()> {
        self.connection
            .simple_query("SELECT 1")
            .await
            .map_err(|_| "metadata execution fence lost; private operator recovery required")?;
        Ok(())
    }
    /// Check before polling a provider call and cancel its future if this session dies.
    /// Dropping a request cannot undo a remote effect: the durable executing marker stays set.
    pub async fn provider<T>(
        &self,
        work: impl std::future::Future<Output = ProviderResult<T>>,
    ) -> ProviderResult<T> {
        self.check().await.map_err(|_| ProviderError::FenceLost)?;
        tokio::select! {
            biased;
            _ = async {
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    if self.check().await.is_err() { break; }
                }
            } => Err(ProviderError::FenceLost),
            result = work => result,
        }
    }

    pub async fn get(&self, id: &Uuid) -> Result<Option<Resource>> {
        let row = self
            .connection
            .query_opt("SELECT body FROM resources WHERE id=$1", &[id])
            .await
            .map_err(|_| "metadata read failed")?;
        row.map(|r| serde_json::from_str(r.get::<_, &str>(0)).map_err(|_| "metadata damaged"))
            .transpose()
    }
    pub async fn save(&self, resource: &Resource) -> Result<()> {
        let body = serde_json::to_string(resource).map_err(|_| "metadata encoding failed")?;
        self.connection.execute("INSERT INTO resources(id,body) VALUES($1,$2) ON CONFLICT(id) DO UPDATE SET body=excluded.body", &[&resource.binding.id, &body])
            .await.map_err(|_| "metadata write failed")?;
        Ok(())
    }
    pub async fn replay(&self, id: &Uuid, fingerprint: &str) -> Result<Option<Value>> {
        let row = self
            .connection
            .query_opt(
                "SELECT fingerprint,result FROM operations WHERE id=$1",
                &[id],
            )
            .await
            .map_err(|_| "operation read failed")?;
        match row {
            None => Ok(None),
            Some(row) => {
                let hash: &str = row.get(0);
                let result: Option<&str> = row.get(1);
                match result {
                    Some(value) if hash == fingerprint => Ok(Some(
                        serde_json::from_str(value).map_err(|_| "operation damaged")?,
                    )),
                    _ => Err("operation changed or needs reconciliation"),
                }
            }
        }
    }
    pub async fn begin(&mut self, id: &Uuid, fingerprint: &str, resource: &Resource) -> Result<()> {
        let transaction = self
            .connection
            .transaction()
            .await
            .map_err(|_| "operation unavailable")?;
        let body = serde_json::to_string(resource).map_err(|_| "metadata encoding failed")?;
        transaction.execute("INSERT INTO resources(id,body) VALUES($1,$2) ON CONFLICT(id) DO UPDATE SET body=excluded.body", &[&resource.binding.id, &body])
            .await.map_err(|_| "metadata write failed")?;
        transaction
            .execute(
                "INSERT INTO operations(id,resource,fingerprint) VALUES($1,$2,$3)",
                &[id, &resource.binding.id, &fingerprint],
            )
            .await
            .map_err(|_| "operation exists")?;
        // This commit MUST complete before any external effect, not at the end of the fence.
        transaction
            .commit()
            .await
            .map_err(|_| "operation unavailable")
    }
    pub async fn finish(&mut self, id: &Uuid, resource: &Resource, result: &Value) -> Result<()> {
        let transaction = self
            .connection
            .transaction()
            .await
            .map_err(|_| "operation unavailable")?;
        let body = serde_json::to_string(resource).map_err(|_| "metadata encoding failed")?;
        let result = result.to_string();
        if transaction
            .execute(
                "UPDATE resources SET body=$2 WHERE id=$1",
                &[&resource.binding.id, &body],
            )
            .await
            .map_err(|_| "metadata write failed")?
            != 1
        {
            return Err("resource missing");
        }
        if transaction
            .execute(
                "UPDATE operations SET result=$2 WHERE id=$1 AND resource=$3 AND result IS NULL",
                &[id, &result, &resource.binding.id],
            )
            .await
            .map_err(|_| "operation write failed")?
            != 1
        {
            return Err("operation missing or completed");
        }
        transaction
            .commit()
            .await
            .map_err(|_| "operation unavailable")
    }
}
