//! Explicit offline migration. Retained history is verified, not silently replayed.
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::BTreeMap;

static LEGACY: include_dir::Dir<'_> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../legacy/control-plane/drizzle");
const NATIVE_BASELINE: &str = include_str!("../schema/baseline.sql");
const RETAINED_BASELINE_SHA256: &str = include_str!("../../../tools/migrations/baseline.sha256");

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("explicit migration target differs from the connected database")]
    Target,
    #[error("ordinary runtime roles cannot migrate")]
    RuntimeRole,
    #[error("retained schema/history is missing; bootstrap a verified empty target first")]
    MissingBaseline,
    #[error("retained migration history differs from this release")]
    History,
    #[error("database migration operation failed")]
    Database,
    #[error("native migration history or execution failed")]
    Native(#[source] sqlx::migrate::MigrateError),
    #[error("bootstrap prerequisites are not satisfied or target is not empty")]
    BootstrapPrerequisite,
    #[error("offline bootstrap operation failed")]
    Bootstrap(#[source] sea_orm::DbErr),
}

pub fn verify_legacy_history(
    expected: &BTreeMap<String, String>,
    applied: &[(String, String)],
) -> Result<(), MigrationError> {
    if expected.len() != applied.len()
        || applied
            .iter()
            .any(|(id, hash)| expected.get(id) != Some(hash))
    {
        return Err(MigrationError::History);
    }
    Ok(())
}

pub fn legacy_checksums() -> BTreeMap<String, String> {
    LEGACY
        .files()
        .filter(|file| file.path().extension().is_some_and(|value| value == "sql"))
        .map(|file| {
            (
                file.path().to_string_lossy().into_owned(),
                hex::encode(Sha256::digest(file.contents())),
            )
        })
        .collect()
}

fn native_migrator() -> sqlx::migrate::Migrator {
    sqlx::migrate!("./migrations")
}

/// Read-only readiness: every native migration must match this exact executable.
pub async fn native_schema_ready(db: &impl ConnectionTrait) -> bool {
    let Ok(rows)=db.query_all_raw(Statement::from_string(DbBackend::Postgres,
        "SELECT version,checksum,success FROM webdock_api_migrations.rust_journal ORDER BY version")).await else {return false;};
    let migrator = native_migrator();
    let required = migrator.iter().collect::<Vec<_>>();
    rows.len() == required.len()
        && rows.iter().zip(required).all(|(row, migration)| {
            row.try_get::<i64>("", "version").ok() == Some(migration.version)
                && row.try_get::<bool>("", "success").ok() == Some(true)
                && row.try_get::<Vec<u8>>("", "checksum").ok().as_deref()
                    == Some(migration.checksum.as_ref())
        })
}

/// Read-only provenance check for initial operator setup. Call under the migration fence.
/// Never stamps a missing marker or repairs ownership/history on an existing database.
pub async fn verify_bootstrap_baseline(db: &impl ConnectionTrait) -> Result<(), MigrationError> {
    let row = db.query_one_raw(Statement::from_string(DbBackend::Postgres,
        "SELECT n.nspowner='webdock_platform_owner'::regrole
          AND (SELECT count(*) FROM pg_catalog.pg_class c WHERE c.relnamespace=n.oid AND c.relname IN ('baseline','journal') AND c.relkind='r' AND c.relowner='webdock_platform_owner'::regrole)=2 AS trusted,
          pg_catalog.to_regclass('webdock_api_migrations.rust_journal') IS NOT NULL AS native_present
         FROM pg_catalog.pg_namespace n WHERE n.nspname='webdock_api_migrations'"
    )).await.map_err(|_| MigrationError::MissingBaseline)?.ok_or(MigrationError::MissingBaseline)?;
    if !row
        .try_get::<bool>("", "trusted")
        .map_err(|_| MigrationError::MissingBaseline)?
    {
        return Err(MigrationError::MissingBaseline);
    }
    let native_present = row
        .try_get::<bool>("", "native_present")
        .map_err(|_| MigrationError::MissingBaseline)?;
    let markers = db
        .query_all_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT id,checksum FROM webdock_api_migrations.baseline",
        ))
        .await
        .map_err(|_| MigrationError::MissingBaseline)?;
    if markers.len() != 1
        || !markers[0]
            .try_get::<bool>("", "id")
            .map_err(|_| MigrationError::MissingBaseline)?
    {
        return Err(MigrationError::MissingBaseline);
    }
    let checksum: String = markers[0]
        .try_get("", "checksum")
        .map_err(|_| MigrationError::MissingBaseline)?;
    let native = checksum == hex::encode(Sha256::digest(NATIVE_BASELINE.as_bytes()));
    if !native && checksum != RETAINED_BASELINE_SHA256.trim() {
        return Err(MigrationError::History);
    }
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT id,checksum FROM webdock_api_migrations.journal ORDER BY id",
        ))
        .await
        .map_err(|_| MigrationError::History)?;
    let applied = rows
        .iter()
        .map(|row| {
            Ok((
                row.try_get::<String>("", "id")
                    .map_err(|_| MigrationError::History)?,
                row.try_get::<String>("", "checksum")
                    .map_err(|_| MigrationError::History)?,
            ))
        })
        .collect::<Result<Vec<_>, MigrationError>>()?;
    verify_legacy_history(&legacy_checksums(), &applied)?;
    if native || native_present {
        let owned = db.query_one_raw(Statement::from_string(DbBackend::Postgres,
            "SELECT relkind='r' AND relowner='webdock_platform_owner'::regrole AS trusted FROM pg_catalog.pg_class WHERE oid=pg_catalog.to_regclass('webdock_api_migrations.rust_journal')"
        )).await.map_err(|_| MigrationError::History)?.ok_or(MigrationError::History)?;
        if !owned
            .try_get::<bool>("", "trusted")
            .map_err(|_| MigrationError::History)?
            || !native_schema_ready(db).await
        {
            return Err(MigrationError::History);
        }
    }
    Ok(())
}

pub async fn bootstrap(
    db: &DatabaseConnection,
    expected_database: &str,
    context_secret: &str,
) -> Result<(), MigrationError> {
    if context_secret.len() < 32 || context_secret.len() > 4096 {
        return Err(MigrationError::BootstrapPrerequisite);
    }
    let tx = db.begin().await.map_err(MigrationError::Bootstrap)?;
    let row = tx
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT current_database() AS database,session_user AS role".to_owned(),
        ))
        .await
        .map_err(MigrationError::Bootstrap)?
        .ok_or(MigrationError::Database)?;
    if row
        .try_get::<String>("", "database")
        .map_err(MigrationError::Bootstrap)?
        != expected_database
    {
        return Err(MigrationError::Target);
    }
    let role = row
        .try_get::<String>("", "role")
        .map_err(MigrationError::Bootstrap)?;
    if role.ends_with("_runtime")
        || role == "webdock_auth_bootstrap"
        || role == "webdock_browser_website"
    {
        return Err(MigrationError::RuntimeRole);
    }
    tx.execute_unprepared("SELECT pg_advisory_xact_lock(hashtext('webdock-api-migration'))")
        .await
        .map_err(MigrationError::Bootstrap)?;
    let present=tx.query_one_raw(Statement::from_string(DbBackend::Postgres,
        "SELECT EXISTS(SELECT FROM pg_namespace WHERE nspname IN ('webdock','webdock_admin','webdock_auth','webdock_mail','webdock_rls','webdock_ids','webdock_api_migrations')) OR EXISTS(SELECT FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_%' AND c.relkind IN ('r','p','v','m','S')) OR EXISTS(SELECT FROM pg_extension WHERE extname='pgcrypto') AS occupied".to_owned())).await.map_err(MigrationError::Bootstrap)?.ok_or(MigrationError::Database)?;
    if present
        .try_get::<bool>("", "occupied")
        .map_err(MigrationError::Bootstrap)?
    {
        return Err(MigrationError::BootstrapPrerequisite);
    }
    let roles=tx.query_one_raw(Statement::from_string(DbBackend::Postgres,
        "SELECT count(*)::bigint AS n FROM pg_roles WHERE rolname IN ('webdock_auth_bootstrap','webdock_auth_runtime','webdock_worker_runtime','webdock_admin_runtime','webdock_registry_runtime','webdock_runtime','webdock_browser_website','webdock_cms_archive_runtime','webdock_platform_owner','webdock_policy_owner','webdock_website_owner') AND NOT (rolsuper OR rolbypassrls OR rolcreatedb OR rolcreaterole OR rolreplication) AND rolcanlogin=(rolname NOT IN ('webdock_platform_owner','webdock_policy_owner','webdock_website_owner'))".to_owned())).await.map_err(MigrationError::Bootstrap)?.ok_or(MigrationError::Database)?;
    if roles
        .try_get::<i64>("", "n")
        .map_err(MigrationError::Bootstrap)?
        != 11
    {
        return Err(MigrationError::BootstrapPrerequisite);
    }
    tx.execute_unprepared(NATIVE_BASELINE)
        .await
        .map_err(MigrationError::Bootstrap)?;
    // This path proved the database empty. Retained upgrades/startup must never stamp fresh state.
    tx.execute_unprepared("INSERT INTO webdock_auth.plugin_bootstrap(mode) VALUES('fresh')")
        .await
        .map_err(MigrationError::Bootstrap)?;
    // The schema-only baseline contains cache triggers but deliberately no data. Their domain
    // registry is structural metadata: derive it from those exact installed triggers before the
    // first customer/user write. Never import customer rows or reset epochs on an existing target.
    tx.execute_unprepared("INSERT INTO webdock_cache.domains(name) SELECT DISTINCT n.nspname||'.'||c.relname FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE NOT t.tgisinternal AND t.tgfoid='webdock_cache.changed()'::regprocedure ON CONFLICT DO NOTHING")
        .await
        .map_err(MigrationError::Bootstrap)?;
    tx.execute_unprepared("CREATE EXTENSION pgcrypto WITH SCHEMA webdock_rls; REVOKE ALL ON ALL FUNCTIONS IN SCHEMA webdock_rls FROM PUBLIC; GRANT EXECUTE ON FUNCTION webdock_rls.hmac(text,text,text) TO webdock_policy_owner").await.map_err(MigrationError::Bootstrap)?;
    tx.execute_unprepared("DO $$ DECLARE f record; BEGIN FOR f IN SELECT p.oid::regprocedure::text AS name FROM pg_proc p JOIN pg_depend d ON d.classid='pg_proc'::regclass AND d.objid=p.oid AND d.deptype='e' JOIN pg_extension e ON e.oid=d.refobjid WHERE e.extname='pgcrypto' LOOP EXECUTE 'ALTER FUNCTION '||f.name||' OWNER TO webdock_policy_owner'; END LOOP; END $$").await.map_err(MigrationError::Bootstrap)?;
    tx.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO webdock_rls.context_key(id,secret) VALUES(true,$1)",
        [context_secret.into()],
    ))
    .await
    .map_err(MigrationError::Bootstrap)?;
    for (id, checksum) in legacy_checksums() {
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO webdock_api_migrations.journal(id,checksum) VALUES($1,$2)",
            [id.into(), checksum.into()],
        ))
        .await
        .map_err(MigrationError::Bootstrap)?;
    }
    tx.execute_unprepared(
        "ALTER SCHEMA webdock_api_migrations OWNER TO webdock_platform_owner;
         REVOKE ALL ON SCHEMA webdock_api_migrations FROM PUBLIC;
         ALTER TABLE webdock_api_migrations.journal OWNER TO webdock_platform_owner;
         ALTER TABLE webdock_api_migrations.rust_journal OWNER TO webdock_platform_owner;
         REVOKE ALL ON webdock_api_migrations.journal,webdock_api_migrations.rust_journal FROM PUBLIC;
         CREATE TABLE webdock_api_migrations.baseline(id boolean PRIMARY KEY CHECK(id),checksum text NOT NULL);
         ALTER TABLE webdock_api_migrations.baseline OWNER TO webdock_platform_owner;
         REVOKE ALL ON webdock_api_migrations.baseline FROM PUBLIC"
    ).await.map_err(MigrationError::Bootstrap)?;
    tx.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO webdock_api_migrations.baseline(id,checksum) VALUES(true,$1)",
        [hex::encode(Sha256::digest(NATIVE_BASELINE.as_bytes())).into()],
    ))
    .await
    .map_err(MigrationError::Bootstrap)?;
    tx.commit().await.map_err(MigrationError::Bootstrap)?;
    upgrade(db, expected_database).await
}

/// Caller supplies a dedicated offline connection and has already verified its exact host.
/// Source databases must be at the retained baseline; unknown/missing history is not guessed.
pub async fn upgrade(
    db: &DatabaseConnection,
    expected_database: &str,
) -> Result<(), MigrationError> {
    let mut connection = db
        .get_postgres_connection_pool()
        .acquire()
        .await
        .map_err(|_| MigrationError::Database)?;
    let result = async {
        let row = sqlx::query("SELECT current_database() AS database,session_user AS role")
            .fetch_one(&mut *connection)
            .await
            .map_err(|_| MigrationError::Database)?;
        if row
            .try_get::<String, _>("database")
            .map_err(|_| MigrationError::Database)?
            != expected_database
        {
            return Err(MigrationError::Target);
        }
        let role = row
            .try_get::<String, _>("role")
            .map_err(|_| MigrationError::Database)?;
        if role.ends_with("_runtime")
            || role == "webdock_auth_bootstrap"
            || role == "webdock_browser_website"
        {
            return Err(MigrationError::RuntimeRole);
        }
        sqlx::query("SELECT pg_advisory_lock(hashtext('webdock-api-migration'))")
            .execute(&mut *connection)
            .await
            .map_err(|_| MigrationError::Database)?;
        let exists: bool =
            sqlx::query_scalar("SELECT to_regclass('webdock_api_migrations.journal') IS NOT NULL")
                .fetch_one(&mut *connection)
                .await
                .map_err(|_| MigrationError::Database)?;
        if !exists {
            return Err(MigrationError::MissingBaseline);
        }
        let rows =
            sqlx::query("SELECT id,checksum FROM webdock_api_migrations.journal ORDER BY id")
                .fetch_all(&mut *connection)
                .await
                .map_err(|_| MigrationError::Database)?;
        let applied: Result<Vec<_>, _> = rows
            .iter()
            .map(|row| {
                Ok((
                    row.try_get::<String, _>("id")?,
                    row.try_get::<String, _>("checksum")?,
                ))
            })
            .collect::<Result<_, sqlx::Error>>();
        verify_legacy_history(
            &legacy_checksums(),
            &applied.map_err(|_| MigrationError::Database)?,
        )?;
        let mut migrator = native_migrator();
        migrator.dangerous_set_table_name("webdock_api_migrations.rust_journal");
        migrator
            .run_direct(None, &mut *connection, false)
            .await
            .map_err(MigrationError::Native)?;
        sqlx::query("REVOKE ALL ON webdock_api_migrations.rust_journal FROM PUBLIC")
            .execute(&mut *connection)
            .await
            .map_err(|_| MigrationError::Database)?;
        sqlx::query("GRANT USAGE ON SCHEMA webdock_api_migrations TO webdock_auth_bootstrap")
            .execute(&mut *connection).await.map_err(|_|MigrationError::Database)?;
        sqlx::query("GRANT SELECT(version,checksum,success) ON webdock_api_migrations.rust_journal TO webdock_auth_bootstrap")
            .execute(&mut *connection).await.map_err(|_|MigrationError::Database)?;
        Ok(())
    }
    .await;
    // Closing releases session-level locks, including on a failed migration.
    connection
        .close()
        .await
        .map_err(|_| MigrationError::Database)?;
    result
}
