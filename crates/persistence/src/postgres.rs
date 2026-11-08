//! Direct driver lifecycle; no SQL, row or ORM compatibility translation.
mod config;
use deadpool_postgres::{Manager, ManagerConfig, Object, RecyclingMethod, Runtime, Timeouts};
use std::{ops::Deref, sync::Arc, time::Duration};

pub(crate) const TRANSACTION_BUDGET: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid native PostgreSQL configuration")]
    Configuration,
    #[error("Native PostgreSQL connection unavailable")]
    Unavailable,
    #[error("Restricted PostgreSQL role or clean session scope required")]
    Authority,
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy)]
pub struct Limits {
    pub max_connections: usize,
    pub min_connections: usize,
    pub wait_timeout: Duration,
    pub connect_timeout: Duration,
    pub idle_timeout: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_connections: 10,
            min_connections: 1,
            wait_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(30),
        }
    }
}
struct Inner {
    pool: deadpool_postgres::Pool,
    role: String,
    limits: Limits,
}
#[derive(Clone)]
pub struct Pool {
    inner: Arc<Inner>,
}
impl Pool {
    pub async fn connect(raw: &str, role: &str, production: bool, limits: Limits) -> Result<Self> {
        if !(1..=32).contains(&limits.max_connections)
            || limits.min_connections > limits.max_connections
            || limits.wait_timeout.is_zero()
            || limits.wait_timeout > Duration::from_secs(300)
            || limits.connect_timeout.is_zero()
            || limits.connect_timeout > Duration::from_secs(60)
            || limits.idle_timeout.is_zero()
            || limits.idle_timeout > Duration::from_secs(3600)
        {
            return Err(Error::Configuration);
        }
        let (config, tls) = config::connection(raw, role, production, limits.connect_timeout)?;
        let manager_config = ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        };
        let manager = match tls {
            Some(tls) => Manager::from_config(config, tls, manager_config),
            None => Manager::from_config(config, tokio_postgres::NoTls, manager_config),
        };
        let pool = deadpool_postgres::Pool::builder(manager)
            .max_size(limits.max_connections)
            .runtime(Runtime::Tokio1)
            .timeouts(Timeouts {
                wait: Some(limits.wait_timeout),
                create: Some(limits.connect_timeout),
                recycle: Some(limits.connect_timeout),
            })
            .build()
            .map_err(|_| Error::Configuration)?;
        let result = Self {
            inner: Arc::new(Inner {
                pool,
                role: role.into(),
                limits,
            }),
        };
        let mut connection = result.acquire().await?;
        connection.transaction().await?.rollback().await?;
        drop(connection);
        // Weak ownership: cleanup cannot keep pools/credentials alive after their caller exits.
        let owner = Arc::downgrade(&result.inner);
        let _idle_cleanup = tokio::spawn(async move {
            loop {
                tokio::time::sleep(limits.idle_timeout).await;
                let Some(inner) = owner.upgrade() else { break };
                let mut remaining = inner.pool.status().size;
                drop(inner.pool.retain(|_, metrics| {
                    if remaining > limits.min_connections
                        && metrics.last_used() >= limits.idle_timeout
                    {
                        remaining -= 1;
                        false
                    } else {
                        true
                    }
                }));
            }
        });
        Ok(result)
    }
    pub async fn acquire(&self) -> Result<Connection> {
        let client = self
            .inner
            .pool
            .get()
            .await
            .map_err(|_| Error::Unavailable)?;
        Ok(Connection {
            owner: self.inner.clone(),
            client: Some(client),
            discard: false,
        })
    }
    pub fn status(&self) -> deadpool_postgres::Status {
        self.inner.pool.status()
    }
}

pub struct Connection {
    owner: Arc<Inner>,
    client: Option<Object>,
    discard: bool,
}
impl Connection {
    // Only this module may start an unchecked transaction. Every public entry verifies authority.
    async fn begin(&mut self) -> Result<Transaction<'_>> {
        if self.discard {
            return Err(Error::Unavailable);
        }
        self.discard = true;
        let deadline = tokio::time::Instant::now() + TRANSACTION_BUDGET;
        let client = self.client.as_mut().ok_or(Error::Unavailable)?;
        // Fresh statements after row-lock acquisition must observe intervening revocation,
        // even when a role/server default has selected a repeatable snapshot.
        let tx = tokio::time::timeout(
            self.owner.limits.connect_timeout,
            client
                .build_transaction()
                .isolation_level(tokio_postgres::IsolationLevel::ReadCommitted)
                .start(),
        )
        .await
        .map_err(|_| Error::Unavailable)?
        .map_err(|_| Error::Unavailable)?;
        Ok(Transaction {
            inner: Some(tx),
            discard: &mut self.discard,
            deadline,
        })
    }
    pub async fn transaction(&mut self) -> Result<Transaction<'_>> {
        let owner = self.owner.clone();
        let tx = self.begin().await?;
        tokio::time::timeout(owner.limits.connect_timeout, verify(&tx, &owner.role))
            .await
            .map_err(|_| Error::Unavailable)??;
        Ok(tx)
    }
    /// Global allocator metadata only: fuse the same authority check with its mandatory row lock.
    pub(crate) async fn allocation(
        &mut self,
        slot: i16,
    ) -> Result<(Transaction<'_>, tokio_postgres::Row)> {
        static SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
            format!(
                "WITH authority AS ({AUTHORITY_SQL}) SELECT g.layout,g.high_water,authority.restricted FROM webdock_ids.generator g CROSS JOIN authority WHERE g.slot=$2 FOR UPDATE OF g"
            )
        });
        let owner = self.owner.clone();
        let tx = self.begin().await?;
        let row = tokio::time::timeout(
            owner.limits.connect_timeout,
            tx.query_typed_opt(
                SQL.as_str(),
                &[
                    (&owner.role, tokio_postgres::types::Type::TEXT),
                    (&slot, tokio_postgres::types::Type::INT2),
                ],
            ),
        )
        .await
        .map_err(|_| Error::Unavailable)?
        .map_err(|_| Error::Unavailable)?
        .ok_or(Error::Unavailable)?;
        if !row
            .try_get::<_, bool>("restricted")
            .map_err(|_| Error::Unavailable)?
        {
            return Err(Error::Authority);
        }
        Ok((tx, row))
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        if self.discard
            && let Some(client) = self.client.take()
        {
            drop(Object::take(client));
        }
    }
}

pub struct Transaction<'a> {
    inner: Option<deadpool_postgres::Transaction<'a>>,
    discard: &'a mut bool,
    deadline: tokio::time::Instant,
}
impl<'a> Deref for Transaction<'a> {
    type Target = tokio_postgres::Transaction<'a>;
    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().expect("live native transaction")
    }
}
impl Transaction<'_> {
    pub async fn commit(mut self) -> Result<()> {
        let tx = self.inner.take().ok_or(Error::Unavailable)?;
        tokio::time::timeout_at(self.deadline, tx.commit())
            .await
            .map_err(|_| Error::Unavailable)?
            .map_err(|_| Error::Unavailable)?;
        *self.discard = false;
        Ok(())
    }
    pub async fn rollback(mut self) -> Result<()> {
        let tx = self.inner.take().ok_or(Error::Unavailable)?;
        tokio::time::timeout_at(self.deadline, tx.rollback())
            .await
            .map_err(|_| Error::Unavailable)?
            .map_err(|_| Error::Unavailable)?;
        *self.discard = false;
        Ok(())
    }
}

const AUTHORITY_SQL:&str="SELECT current_user::text=$1 AND session_user::text=$1 AND b.usesysid=r.oid
        AND NOT(r.rolsuper OR r.rolbypassrls OR r.rolcreatedb OR r.rolcreaterole OR r.rolreplication)
        AND NOT pg_catalog.pg_has_role(current_user,'webdock_platform_owner','MEMBER')
        AND NOT pg_catalog.pg_has_role(current_user,'webdock_policy_owner','MEMBER')
        AND NOT pg_catalog.pg_has_role(current_user,'webdock_website_owner','MEMBER')
        AND coalesce(pg_catalog.current_setting('webdock.rls_context',true),'')=''
        AND coalesce(pg_catalog.current_setting('webdock.rls_signature',true),'')='' AS restricted,
        pg_catalog.set_config('statement_timeout','30000',true),pg_catalog.set_config('lock_timeout','5000',true)
        FROM pg_catalog.pg_roles r JOIN pg_catalog.pg_stat_get_activity(pg_catalog.pg_backend_pid()) b ON b.pid=pg_catalog.pg_backend_pid()
        WHERE r.rolname=current_user";

async fn verify(tx: &tokio_postgres::Transaction<'_>, role: &str) -> Result<()> {
    let row = tx
        .query_typed_one(AUTHORITY_SQL, &[(&role, tokio_postgres::types::Type::TEXT)])
        .await
        .map_err(|_| Error::Unavailable)?;
    if row
        .try_get::<_, bool>("restricted")
        .map_err(|_| Error::Unavailable)?
    {
        Ok(())
    } else {
        Err(Error::Authority)
    }
}
