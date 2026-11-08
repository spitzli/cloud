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
    webdock_persistence::connection::verify_runtime_role(&db, expected)
        .await
        .map_err(|_| "Restricted independent runtime role required")?;
    Ok(db)
}

pub async fn require_runtime_role(
    db: &DatabaseConnection,
    expected: &str,
) -> crate::runtime::Result<()> {
    webdock_persistence::connection::verify_runtime_role(db, expected)
        .await
        .map_err(|_| crate::runtime::Error::Message("Restricted database role required".into()))
}

pub async fn connect_runtime(
    url: &str,
    expected: &str,
    production: bool,
) -> crate::runtime::Result<DatabaseConnection> {
    let url =
        webdock_persistence::connection::ConnectionUrl::parse(url, production).map_err(|_| {
            crate::runtime::Error::Message("Invalid PostgreSQL connection policy".into())
        })?;
    let mut options = ConnectOptions::new(url.as_str());
    options
        .max_connections(10)
        .min_connections(1)
        .sqlx_logging(false);
    let db = Database::connect(options)
        .await
        .map_err(|_| crate::runtime::Error::Message("Runtime database connection failed".into()))?;
    require_runtime_role(&db, expected).await?;
    Ok(db)
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
