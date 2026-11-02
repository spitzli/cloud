//! Additive dedicated-database readonly export role. Existing backup wire contracts are unchanged.
//! The executor calls create in postgres, grants in the exact database, then sets SCRAM via stdin.
pub(super) struct ExportRole {
    pub name: String,
    pub create: String,
    pub grants: String,
    pub enable: String,
    pub disable: String,
    pub drop_role: String,
}
impl ExportRole {
    pub fn new(id: &str, database: &str, kind: &str) -> Result<Option<Self>, &'static str> {
        if kind != "dedicated" {
            return Ok(None);
        }
        if !webdock_export_protocol::decimal(id) || database != format!("db_{id}") {
            return Err("Invalid dedicated export database");
        }
        let name = format!("wd_export_{id}");
        let migration = format!("wd_migrate_{id}");
        let marker = format!("webdock:database:{id}");
        let check = format!(
            "IF (SELECT pg_catalog.shobj_description(oid,'pg_authid') FROM pg_catalog.pg_roles WHERE rolname='{name}') IS DISTINCT FROM '{marker}' THEN RAISE EXCEPTION 'Export role ownership conflict'; END IF;"
        );
        let create = format!(
            "DO $webdock$ BEGIN IF NOT EXISTS(SELECT 1 FROM pg_catalog.pg_roles WHERE rolname='{name}') THEN CREATE ROLE \"{name}\" NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS NOINHERIT CONNECTION LIMIT 2; COMMENT ON ROLE \"{name}\" IS '{marker}'; ELSE {check} END IF; END $webdock$; DO $webdock$ BEGIN IF EXISTS(SELECT 1 FROM pg_catalog.pg_auth_members WHERE member=(SELECT oid FROM pg_catalog.pg_roles WHERE rolname='{name}')) THEN RAISE EXCEPTION 'Export role membership conflict'; END IF; END $webdock$; REVOKE ALL ON DATABASE \"{database}\" FROM \"{name}\"; GRANT CONNECT ON DATABASE \"{database}\" TO \"{name}\"; ALTER ROLE \"{name}\" SET default_transaction_read_only=on; ALTER ROLE \"{name}\" SET statement_timeout='20s'; ALTER ROLE \"{name}\" SET lock_timeout='5s';"
        );
        let grants = format!(
            "REVOKE ALL ON SCHEMA app FROM \"{name}\"; GRANT USAGE ON SCHEMA app TO \"{name}\"; REVOKE ALL ON ALL TABLES IN SCHEMA app FROM \"{name}\"; GRANT SELECT ON ALL TABLES IN SCHEMA app TO \"{name}\"; REVOKE ALL ON ALL SEQUENCES IN SCHEMA app FROM \"{name}\"; GRANT SELECT ON ALL SEQUENCES IN SCHEMA app TO \"{name}\"; ALTER DEFAULT PRIVILEGES FOR ROLE \"{migration}\" IN SCHEMA app GRANT SELECT ON TABLES TO \"{name}\"; ALTER DEFAULT PRIVILEGES FOR ROLE \"{migration}\" IN SCHEMA app GRANT SELECT ON SEQUENCES TO \"{name}\"; ALTER ROLE \"{name}\" IN DATABASE \"{database}\" SET search_path=app,pg_catalog;"
        );
        let enable = format!(
            "ALTER ROLE \"{name}\" NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS NOINHERIT LOGIN CONNECTION LIMIT 2;"
        );
        let disable = format!(
            "DO $webdock$ BEGIN IF EXISTS(SELECT 1 FROM pg_catalog.pg_roles WHERE rolname='{name}') THEN {check} ALTER ROLE \"{name}\" NOLOGIN; PERFORM pg_catalog.pg_terminate_backend(pid) FROM pg_catalog.pg_stat_activity WHERE usename='{name}' AND pid<>pg_catalog.pg_backend_pid(); END IF; END $webdock$;"
        );
        let drop_role = format!(
            "DO $webdock$ BEGIN IF EXISTS(SELECT 1 FROM pg_catalog.pg_roles WHERE rolname='{name}') THEN {check} DROP ROLE \"{name}\"; END IF; END $webdock$;"
        );
        Ok(Some(Self {
            name,
            create,
            grants,
            enable,
            disable,
            drop_role,
        }))
    }
    pub fn password(id: &str, application_password: &str) -> Result<String, &'static str> {
        webdock_export_protocol::readonly_password(id, application_password)
    }
}
