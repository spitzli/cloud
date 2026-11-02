//! Bounded app-only pg_dump; production material comes exclusively from Core source authority.
use crate::{MAX_BYTES, Result};
use serde::Deserialize;
use std::{path::Path, process::Stdio};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseSource {
    pub customer: String,
    pub project: String,
    pub url: String,
}
/// Remote material only comes from the private Core source RPC, never user-selected URL/SQL.
pub async fn dump_managed(
    material: webdock_export_protocol::wire::DatabaseMaterial,
    root: &Path,
) -> Result<tokio::fs::File> {
    use tokio::io::AsyncWriteExt;
    if material.host.is_empty()
        || material.host.len() > 253
        || !material
            .host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        || material.port == 0
        || material.port > 65535
        || material.expires_at <= crate::now()
        || material.ca_certificate.len() > 262144
        || material.ca_certificate.is_empty()
    {
        return Err("Invalid managed source");
    }
    let mut uri =
        url::Url::parse("postgresql://localhost").map_err(|_| "Invalid managed source")?;
    uri.set_host(Some(&material.host))
        .map_err(|_| "Invalid managed source")?;
    uri.set_port(Some(material.port as u16))
        .map_err(|_| "Invalid managed source")?;
    uri.set_path(&material.database);
    uri.set_username(&material.username)
        .map_err(|_| "Invalid managed source")?;
    uri.set_password(Some(&material.password))
        .map_err(|_| "Invalid managed source")?;
    let ca = tempfile::NamedTempFile::new_in(root)
        .map_err(|_| "Private source trust staging unavailable")?;
    std::fs::write(ca.path(), material.ca_certificate.as_bytes())
        .map_err(|_| "Private source trust staging unavailable")?;
    let client = webdock_plugin_kit::postgres::connect(
        uri.as_str(),
        Some(material.ca_certificate.as_bytes()),
    )
    .await
    .map_err(|_| "Readonly source role is not reconciled or TLS verification failed")?;
    let check=client.query_one("SELECT current_user::text,current_database()::text,NOT(rolsuper OR rolcreaterole OR rolcreatedb OR rolreplication OR rolbypassrls OR rolinherit),NOT EXISTS(SELECT 1 FROM pg_auth_members WHERE member=r.oid),NOT has_database_privilege(current_user,current_database(),'CREATE,TEMP'),NOT EXISTS(SELECT 1 FROM pg_namespace n WHERE n.nspname NOT LIKE 'pg_%' AND n.nspname<>'information_schema' AND has_schema_privilege(current_user,n.oid,'CREATE')),NOT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname NOT LIKE 'pg_%' AND n.nspname<>'information_schema' AND c.relkind IN ('r','p','v','m','f') AND has_table_privilege(current_user,c.oid,'INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')) FROM pg_roles r WHERE rolname=current_user",&[]).await.map_err(|_|"Readonly source validation failed")?;
    if check.get::<_, String>(0) != material.username
        || check.get::<_, String>(1) != material.database
        || (2..7).any(|i| !check.get::<_, bool>(i))
    {
        return Err("Source role has unsafe privileges");
    }
    drop(client);
    let output = tempfile::tempfile_in(root).map_err(|_| "Private export staging unavailable")?;
    let mut output = tokio::fs::File::from_std(output);
    let mut command = tokio::process::Command::new("/usr/bin/pg_dump");
    command
        .env_clear()
        .env("PGHOST", &material.host)
        .env("PGPORT", material.port.to_string())
        .env("PGUSER", &material.username)
        .env("PGPASSWORD", &material.password)
        .env("PGDATABASE", &material.database)
        .env("PGSSLMODE", "verify-full")
        .env("PGSSLROOTCERT", ca.path())
        .env("PGAPPNAME", "webdock_archives_export")
        .env("PGCONNECT_TIMEOUT", "3")
        .env(
            "PGOPTIONS",
            "-c default_transaction_read_only=on -c statement_timeout=20000",
        )
        .args([
            "--format=custom",
            "--compress=gzip:6",
            "--schema=app",
            "--no-owner",
            "--no-privileges",
            "--no-password",
            "--lock-wait-timeout=5s",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| "Managed database export unavailable")?;
    let mut input = child
        .stdout
        .take()
        .ok_or("Managed export unavailable")?
        .take(MAX_BYTES + 1);
    let seconds = (material.expires_at - crate::now()).clamp(1, 25) as u64;
    tokio::time::timeout(std::time::Duration::from_secs(seconds), async {
        let n = tokio::io::copy(&mut input, &mut output)
            .await
            .map_err(|_| "Managed export stream failed")?;
        if n > MAX_BYTES {
            return Err("Managed export limit");
        }
        if !child
            .wait()
            .await
            .map_err(|_| "Managed export failed")?
            .success()
        {
            return Err("Managed export failed");
        }
        output
            .flush()
            .await
            .map_err(|_| "Managed export staging failed")?;
        output
            .rewind()
            .await
            .map_err(|_| "Managed export staging failed")?;
        Ok(())
    })
    .await
    .map_err(|_| "Managed export deadline exceeded")??;
    Ok(output)
}
