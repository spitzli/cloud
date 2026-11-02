//! Private receipts, not an execution queue. Core owns job leases and actor authority.
use crate::{Result, model::Archive};
use tokio_postgres::Client;
use uuid::Uuid;
pub struct Store {
    url: String,
    ca: Option<Vec<u8>>,
}
pub struct Fence {
    client: Client,
}
impl Store {
    pub async fn connect(url: &str, ca: Option<&[u8]>) -> Result<Self> {
        let migrations = [include_str!("../migrations/0001_metadata.sql")];
        let client = match ca {
            Some(ca) => webdock_plugin_kit::storage_tls(url, ca, "archives", &migrations)
                .await
                .map_err(|_| "Archive metadata unavailable")?,
            None => {
                webdock_plugin_kit::loopback(url).map_err(|_| "Private metadata TLS required")?;
                webdock_plugin_kit::storage(url, "archives", &migrations)
                    .await
                    .map_err(|_| "Archive metadata unavailable")?
            }
        };
        let version: i32 = client
            .query_one("SELECT max(version) FROM schema_migrations", &[])
            .await
            .map_err(|_| "Archive schema unavailable")?
            .get(0);
        if version != 1 {
            return Err("Archive schema is newer than runtime");
        }
        Ok(Self {
            url: url.into(),
            ca: ca.map(Vec::from),
        })
    }
    async fn connection(&self) -> Result<Client> {
        let c = webdock_plugin_kit::postgres::connect(&self.url, self.ca.as_deref())
            .await
            .map_err(|_| "Archive metadata unavailable")?;
        c.batch_execute("SET statement_timeout='500ms'; SET synchronous_commit=on")
            .await
            .map_err(|_| "Archive metadata unavailable")?;
        Ok(c)
    }
    pub async fn read(&self, id: Uuid) -> Result<Option<Archive>> {
        let c = self.connection().await?;
        read(&c, id).await
    }
    pub async fn lock(&self, id: Uuid) -> Result<Fence> {
        let c = self.connection().await?;
        let locked: bool = c
            .query_one(
                "SELECT pg_try_advisory_lock(hashtextextended($1,70461025))",
                &[&id.to_string()],
            )
            .await
            .map_err(|_| "Archive fence unavailable")?
            .get(0);
        if !locked {
            return Err("Archive operation in progress");
        }
        Ok(Fence { client: c })
    }
}
async fn read(c: &Client, id: Uuid) -> Result<Option<Archive>> {
    c.query_opt("SELECT body FROM archives WHERE id=$1", &[&id])
        .await
        .map_err(|_| "Archive metadata unavailable")?
        .map(|r| serde_json::from_str(r.get(0)).map_err(|_| "Archive metadata damaged"))
        .transpose()
}
impl Fence {
    pub async fn read(&self, id: Uuid) -> Result<Option<Archive>> {
        read(&self.client, id).await
    }
    pub async fn create(&self, a: &Archive, hash: &str) -> Result<()> {
        let body = serde_json::to_string(a).map_err(|_| "Archive metadata damaged")?;
        self.client.execute("INSERT INTO archives(id,request_hash,body) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",&[&a.id,&hash,&body]).await.map_err(|_|"Archive creation unavailable")?;
        let row = self
            .client
            .query_one("SELECT request_hash FROM archives WHERE id=$1", &[&a.id])
            .await
            .map_err(|_| "Archive creation unavailable")?;
        if row.get::<_, String>(0) != hash {
            return Err("Archive request conflicts");
        }
        Ok(())
    }
    pub async fn save(&self, a: &Archive) -> Result<()> {
        let body = serde_json::to_string(a).map_err(|_| "Archive metadata damaged")?;
        let changed = self
            .client
            .execute("UPDATE archives SET body=$2 WHERE id=$1", &[&a.id, &body])
            .await
            .map_err(|_| "Archive completion unavailable")?;
        if changed != 1 {
            return Err("Archive completion unavailable");
        }
        Ok(())
    }
}
