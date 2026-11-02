use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement,
    TransactionTrait,
};
use webdock_ids::{Id, Identity, Layout};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ID allocation is unavailable")]
    Unavailable,
    #[error("database allocation failed")]
    Database(#[from] sea_orm::DbErr),
    #[error("invalid allocation state")]
    Id(#[from] webdock_ids::Error),
}

pub struct IdAllocator {
    db: DatabaseConnection,
    identity: Identity,
}
impl IdAllocator {
    /// Use a dedicated pool, not a pool whose callers hold business transactions.
    pub fn new(db: DatabaseConnection, identity: Identity) -> Self {
        Self { db, identity }
    }
    pub async fn connect(url: &str, identity: Identity) -> Result<Self, Error> {
        let url = connection::ConnectionUrl::parse(url, false).map_err(|_| Error::Unavailable)?;
        let mut options = ConnectOptions::new(url.as_str());
        options
            .max_connections(2)
            .min_connections(1)
            .sqlx_logging(false);
        Ok(Self::new(Database::connect(options).await?, identity))
    }
    pub async fn allocate(&self) -> Result<Id, Error> {
        self.allocate_inner(None).await
    }
    pub async fn allocate_at(&self, now_ms: u64) -> Result<Id, Error> {
        self.allocate_inner(Some(now_ms)).await
    }
    async fn allocate_inner(&self, now_ms: Option<u64>) -> Result<Id, Error> {
        Identity::platform(self.identity.machine(), self.identity.generator())?;
        let slot = ((self.identity.machine() << 4) | self.identity.generator()) as i16;
        let tx = self.db.begin().await?;
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT layout,high_water FROM webdock_ids.generator WHERE slot=$1 FOR UPDATE",
                [slot.into()],
            ))
            .await?
            .ok_or(Error::Unavailable)?;
        if row.try_get::<String>("", "layout")? != "2026-01-01:41:6:4:12" {
            return Err(Error::Unavailable);
        }
        let previous = row
            .try_get::<Option<i64>>("", "high_water")?
            .map(Id::try_from)
            .transpose()?;
        let now = if let Some(now) = now_ms {
            now
        } else {
            let time = tx
                .query_one_raw(Statement::from_string(
                    DbBackend::Postgres,
                    "SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint AS now_ms"
                        .to_owned(),
                ))
                .await?
                .ok_or(Error::Unavailable)?
                .try_get::<i64>("", "now_ms")?;
            u64::try_from(time).map_err(|_| Error::Unavailable)?
        };
        let next = Layout::PLATFORM.next_after(self.identity, previous, now)?;
        let updated = tx
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE webdock_ids.generator SET high_water=$1 WHERE slot=$2",
                [next.as_i64().into(), slot.into()],
            ))
            .await?;
        if updated.rows_affected() != 1 {
            return Err(Error::Unavailable);
        }
        tx.commit().await?;
        Ok(next)
    }
}
pub mod migrations;

pub mod connection;
pub mod roles;
