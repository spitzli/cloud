use sea_orm::{ConnectOptions, Database, DatabaseConnection};

/// Binding resolution may run inside a host transaction; it must not borrow that transaction's pool.
pub async fn independent_pool(
    source: &DatabaseConnection,
    expected: &str,
) -> Result<DatabaseConnection, crate::plugin_config::ConfigurationError> {
    let options = source.get_postgres_connection_pool().connect_options();
    let pool = sea_orm::sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .min_connections(1)
        .connect_with((*options).clone())
        .await
        .map_err(|_| "Independent runtime pool unavailable")?;
    let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool);
    spitzli_persistence::connection::verify_runtime_role(&db, expected)
        .await
        .map_err(|_| "Restricted independent runtime role required")?;
    Ok(db)
}

pub async fn require_runtime_role(
    db: &DatabaseConnection,
    expected: &str,
) -> crate::runtime::Result<()> {
    spitzli_persistence::connection::verify_runtime_role(db, expected)
        .await
        .map_err(|_| crate::runtime::Error::Message("Restricted database role required".into()))
}

pub async fn connect_runtime(
    url: &str,
    expected: &str,
    production: bool,
) -> crate::runtime::Result<DatabaseConnection> {
    let url =
        spitzli_persistence::connection::ConnectionUrl::parse(url, production).map_err(|_| {
            crate::runtime::Error::Message("Invalid PostgreSQL connection policy".into())
        })?;
    let limits = RuntimeLimits::from_env();
    let mut options = ConnectOptions::new(url.as_str());
    options
        .max_connections(limits.max_connections)
        .min_connections(limits.min_connections)
        .acquire_timeout(limits.acquire_timeout)
        .idle_timeout(limits.idle_timeout)
        .sqlx_logging(false);
    let db = Database::connect(options)
        .await
        .map_err(|_| crate::runtime::Error::Message("Runtime database connection failed".into()))?;
    require_runtime_role(&db, expected).await?;
    Ok(db)
}

/// Runtime pool limits; overridable per deployment without a rebuild.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeLimits {
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout: std::time::Duration,
    pub idle_timeout: std::time::Duration,
}

impl RuntimeLimits {
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let num = |key: &str, default: u64| {
            get(key)
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(default)
        };
        let max = num("SPITZLI_RUNTIME_DB_MAX_CONNECTIONS", 20).clamp(1, 200) as u32;
        Self {
            max_connections: max,
            min_connections: num("SPITZLI_RUNTIME_DB_MIN_CONNECTIONS", 1).min(u64::from(max))
                as u32,
            acquire_timeout: std::time::Duration::from_secs(
                num("SPITZLI_RUNTIME_DB_ACQUIRE_TIMEOUT_SECS", 5).clamp(1, 60),
            ),
            idle_timeout: std::time::Duration::from_secs(
                num("SPITZLI_RUNTIME_DB_IDLE_TIMEOUT_SECS", 300).clamp(10, 3600),
            ),
        }
    }
}

/// Shares one pool per (URL, role) across services instead of opening a pool per service.
#[derive(Default)]
pub struct RuntimePools {
    pools: tokio::sync::Mutex<std::collections::HashMap<(String, String), DatabaseConnection>>,
}

impl RuntimePools {
    pub async fn get(
        &self,
        url: &str,
        expected: &str,
        production: bool,
    ) -> crate::runtime::Result<DatabaseConnection> {
        let key = (url.to_owned(), expected.to_owned());
        let mut pools = self.pools.lock().await;
        if let Some(db) = pools.get(&key) {
            return Ok(db.clone());
        }
        let db = connect_runtime(url, expected, production).await?;
        pools.insert(key, db.clone());
        Ok(db)
    }
}

/// Preserve the previous bootstrap connection policy without framework initialization.
pub async fn connect_bootstrap(
    config: &crate::config::RuntimeConfig,
) -> crate::runtime::Result<DatabaseConnection> {
    let database = &config.database;
    let mut options = ConnectOptions::new(database.uri.as_str());
    options
        .min_connections(database.min_connections)
        .max_connections(database.max_connections)
        .connect_timeout(database.connect_timeout)
        .acquire_timeout(database.acquire_timeout)
        .idle_timeout(database.idle_timeout)
        .sqlx_logging(false);
    let db = Database::connect(options)
        .await
        .map_err(|_| crate::runtime::Error::Message("Runtime database connection failed".into()))?;
    require_runtime_role(&db, "webdock_auth_bootstrap").await?;
    Ok(db)
}

#[cfg(test)]
mod runtime_limits_tests {
    use super::RuntimeLimits;
    use std::time::Duration;

    #[test]
    fn defaults_and_overrides() {
        let d = RuntimeLimits::from_lookup(|_| None);
        assert_eq!((d.max_connections, d.min_connections), (20, 1));
        assert_eq!(d.acquire_timeout, Duration::from_secs(5));
        let o = RuntimeLimits::from_lookup(|k| match k {
            "SPITZLI_RUNTIME_DB_MAX_CONNECTIONS" => Some("3".into()),
            "SPITZLI_RUNTIME_DB_MIN_CONNECTIONS" => Some("9".into()),
            "SPITZLI_RUNTIME_DB_ACQUIRE_TIMEOUT_SECS" => Some("bad".into()),
            _ => None,
        });
        assert_eq!((o.max_connections, o.min_connections), (3, 3));
        assert_eq!(o.acquire_timeout, Duration::from_secs(5));
    }
}
