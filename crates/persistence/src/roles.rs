//! Explicit fresh-install role provisioning. Existing role credentials are never changed.
use crate::migrations::MigrationError;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use std::collections::BTreeMap;
pub const LOGIN_ROLES: &[&str] = &[
    "webdock_auth_bootstrap",
    "webdock_auth_runtime",
    "webdock_worker_runtime",
    "webdock_admin_runtime",
    "webdock_registry_runtime",
    "webdock_runtime",
    "webdock_browser_website",
    "webdock_cms_archive_runtime",
];
/// Deliberately excluded from runtime roles: this login can explicitly assume schema owners.
pub const OFFLINE_ROLE: &str = "webdock_offline_migrate";
const OWNER_ROLES: &[&str] = &[
    "webdock_platform_owner",
    "webdock_policy_owner",
    "webdock_website_owner",
];
pub fn validate_passwords(passwords: &BTreeMap<String, String>) -> Result<(), MigrationError> {
    if !(passwords.len() == LOGIN_ROLES.len()
        || passwords.len() == LOGIN_ROLES.len() + 1 && passwords.contains_key(OFFLINE_ROLE))
        || LOGIN_ROLES
            .iter()
            .any(|role| !passwords.contains_key(*role))
        || passwords.iter().any(|(role, password)| {
            !LOGIN_ROLES.contains(&role.as_str()) && role != OFFLINE_ROLE || {
                password.len() < 32
                    || password.len() > 256
                    || !password
                        .bytes()
                        .all(|v| v.is_ascii_alphanumeric() || b"-_".contains(&v))
            }
        })
    {
        return Err(MigrationError::BootstrapPrerequisite);
    }
    Ok(())
}
/// Requires an explicit superuser connection to the verified EMPTY target. Runtime never calls this.
/// Pass a connection with SQL logging disabled: CREATE ROLE contains private credential material.
pub async fn provision_roles(
    db: &DatabaseConnection,
    target: &str,
    passwords: &BTreeMap<String, String>,
) -> Result<(), MigrationError> {
    validate_passwords(passwords)?;
    let tx = db.begin().await.map_err(|_| MigrationError::Database)?;
    let row=tx.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT current_database() AS database,rolsuper AS allowed FROM pg_roles WHERE rolname=session_user")).await.map_err(|_|MigrationError::Database)?.ok_or(MigrationError::Database)?;
    if row
        .try_get::<String>("", "database")
        .map_err(|_| MigrationError::Database)?
        != target
    {
        return Err(MigrationError::Target);
    }
    if !row
        .try_get::<bool>("", "allowed")
        .map_err(|_| MigrationError::Database)?
    {
        return Err(MigrationError::RuntimeRole);
    }
    tx.execute_unprepared("SELECT pg_advisory_xact_lock(hashtext('webdock-role-bootstrap'))")
        .await
        .map_err(|_| MigrationError::Database)?;
    let row=tx.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_%' AND c.relkind IN ('r','p','v','m','S')) OR EXISTS(SELECT FROM pg_namespace WHERE nspname LIKE 'webdock%') AS occupied")).await.map_err(|_|MigrationError::Database)?.ok_or(MigrationError::Database)?;
    if row
        .try_get::<bool>("", "occupied")
        .map_err(|_| MigrationError::Database)?
    {
        return Err(MigrationError::BootstrapPrerequisite);
    }
    for role in OWNER_ROLES.iter().chain(LOGIN_ROLES) {
        let login = LOGIN_ROLES.contains(role);
        let existing=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT rolcanlogin,NOT (rolsuper OR rolbypassrls OR rolcreatedb OR rolcreaterole OR rolreplication OR rolinherit) AND NOT EXISTS(SELECT FROM pg_auth_members m WHERE m.member=r.oid) AS restricted FROM pg_roles r WHERE rolname=$1",[(*role).into()])).await.map_err(|_|MigrationError::Database)?;
        if let Some(existing) = existing {
            if existing
                .try_get::<bool>("", "rolcanlogin")
                .map_err(|_| MigrationError::Database)?
                != login
                || !existing
                    .try_get::<bool>("", "restricted")
                    .map_err(|_| MigrationError::Database)?
            {
                return Err(MigrationError::BootstrapPrerequisite);
            }
            continue;
        }
        // Identifiers are fixed above; passwords are strictly base64url/hex-safe ASCII.
        let authentication = if login {
            format!("LOGIN PASSWORD '{}'", passwords[*role])
        } else {
            "NOLOGIN".into()
        };
        tx.execute_unprepared(&format!("CREATE ROLE {role} {authentication} NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION")).await.map_err(|_|MigrationError::Database)?;
    }
    if let Some(password) = passwords.get(OFFLINE_ROLE) {
        let existing = tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT r.rolcanlogin AND NOT(r.rolsuper OR r.rolbypassrls OR r.rolcreatedb OR r.rolcreaterole OR r.rolreplication OR r.rolinherit)
              AND (SELECT count(*) FROM pg_catalog.pg_auth_members m WHERE m.member=r.oid)=3
              AND NOT EXISTS(SELECT FROM pg_catalog.pg_auth_members m JOIN pg_catalog.pg_roles parent ON parent.oid=m.roleid WHERE m.member=r.oid AND
                (parent.rolname NOT IN ('webdock_platform_owner','webdock_policy_owner','webdock_website_owner') OR m.admin_option OR m.inherit_option OR NOT m.set_option)) AS restricted
             FROM pg_catalog.pg_roles r WHERE r.rolname=$1",
            [OFFLINE_ROLE.into()],
        )).await.map_err(|_| MigrationError::Database)?;
        if let Some(existing) = existing {
            if !existing
                .try_get::<bool>("", "restricted")
                .map_err(|_| MigrationError::Database)?
            {
                return Err(MigrationError::BootstrapPrerequisite);
            }
        } else {
            tx.execute_unprepared(&format!(
                "CREATE ROLE {OFFLINE_ROLE} LOGIN PASSWORD '{password}' NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION"
            )).await.map_err(|_| MigrationError::Database)?;
            for owner in OWNER_ROLES {
                tx.execute_unprepared(&format!(
                    "GRANT {owner} TO {OFFLINE_ROLE} WITH ADMIN FALSE, INHERIT FALSE, SET TRUE"
                ))
                .await
                .map_err(|_| MigrationError::Database)?;
            }
        }
    }
    tx.commit().await.map_err(|_| MigrationError::Database)
}
