//! This administrator must target the separate plugin metadata service, never platform PostgreSQL.
//! The worker checks observed cluster identity before calling this helper's provisioning DDL.
use super::{ProvisioningError as Error, Result};
use sha2::{Digest, Sha256};
use tokio_postgres::Client;

/// All generated material is supplied from the durable install plan. Retries generate nothing.
pub struct StoragePlan {
    database: String,
    role: String,
    marker: String,
    password: String,
    host: String,
    port: u16,
}
impl StoragePlan {
    pub fn new(
        platform: &str,
        plugin: &str,
        password: String,
        host: String,
        port: u16,
    ) -> Result<Self> {
        if platform.is_empty()
            || platform.len() > 128
            || !crate::slug(plugin)
            || password.len() < 32
            || password.len() > 256
            || password.chars().any(char::is_control)
            || host.is_empty()
            || host.len() > 253
            || port == 0
        {
            return Err(Error::Invalid);
        }
        let identity = hex::encode(Sha256::digest(
            serde_json::to_vec(&(platform, plugin)).map_err(|_| Error::Invalid)?,
        ));
        let marker = format!(
            "webdock-plugin-metadata:v1:{identity}:{}",
            hex::encode(Sha256::digest(password.as_bytes()))
        );
        let plan = Self {
            database: format!("wd_pm_d_{}", &identity[..32]),
            role: format!("wd_pm_r_{}", &identity[..32]),
            marker,
            password,
            host,
            port,
        };
        plan.url()?;
        Ok(plan)
    }
    pub fn database(&self) -> &str {
        &self.database
    }
    pub fn role(&self) -> &str {
        &self.role
    }
    fn url(&self) -> Result<String> {
        let mut url = url::Url::parse("postgresql://localhost/").map_err(|_| Error::Invalid)?;
        url.set_host(Some(&self.host)).map_err(|_| Error::Invalid)?;
        url.set_port(Some(self.port)).map_err(|_| Error::Invalid)?;
        url.set_username(&self.role).map_err(|_| Error::Invalid)?;
        url.set_password(Some(&self.password))
            .map_err(|_| Error::Invalid)?;
        url.set_path(&self.database);
        Ok(url.into())
    }
}
/// Private application connection only. No Debug/Serialize: credentials cannot enter diagnostics.
pub struct ProvisionedStorage {
    pub url: String,
    database: String,
    role: String,
}
impl ProvisionedStorage {
    pub fn database(&self) -> &str {
        &self.database
    }
    pub fn role(&self) -> &str {
        &self.role
    }
}
pub struct MetadataProvisioner {
    client: tokio::sync::Mutex<Client>,
    tls: bool,
}
/// PostgreSQL's actual initdb identity, shared across databases and listening interfaces.
/// Opaque and equality-only so endpoint strings cannot accidentally stand in for this value.
#[derive(Clone, PartialEq, Eq)]
pub struct ClusterIdentity(String);
impl ClusterIdentity {
    pub fn parse(value: String) -> Result<Self> {
        let id = value.parse::<i64>().map_err(|_| Error::Trust)?;
        if id == 0 || id.to_string() != value {
            return Err(Error::Trust);
        }
        Ok(Self(value))
    }
}
fn ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
impl MetadataProvisioner {
    /// Read only the public control-data identifier; no monitoring role or privilege escalation.
    /// Missing permission or unavailable control data fails closed.
    pub async fn cluster_identity(&self) -> Result<ClusterIdentity> {
        let client = self.client.lock().await;
        let row = client
            .query_one(
                "SELECT system_identifier::pg_catalog.text FROM pg_catalog.pg_control_system()",
                &[],
            )
            .await
            .map_err(|_| Error::Unavailable)?;
        ClusterIdentity::parse(row.try_get(0).map_err(|_| Error::Unavailable)?)
    }
    pub async fn require_separate_cluster(&self, platform: &ClusterIdentity) -> Result<()> {
        if self.cluster_identity().await? == *platform {
            return Err(Error::Ownership);
        }
        Ok(())
    }
    /// Diagnostic TCP endpoint only. Different interfaces can belong to the same cluster;
    /// use require_separate_cluster for the installer's storage separation boundary.
    pub async fn server_identity(&self) -> Result<(String, u16)> {
        let client = self.client.lock().await;
        let row = client
            .query_one("SELECT inet_server_addr()::text,inet_server_port()", &[])
            .await
            .map_err(|_| Error::Unavailable)?;
        let address: String = row.get::<_, Option<String>>(0).ok_or(Error::Trust)?;
        // PostgreSQL's inet-to-text includes the host mask (e.g. /32 or /128). Keep
        // that exact server value so the platform connection can compare the same query.
        address
            .split('/')
            .next()
            .ok_or(Error::Trust)?
            .parse::<std::net::IpAddr>()
            .map_err(|_| Error::Trust)?;
        let port = row
            .get::<_, Option<i32>>(1)
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p != 0)
            .ok_or(Error::Trust)?;
        Ok((address, port))
    }
    /// `None` is reserved for explicit loopback fixtures. Remote administrators always use CA TLS.
    pub async fn connect(admin_url: &str, ca: Option<&[u8]>) -> Result<Self> {
        let client = webdock_plugin_kit::postgres::connect(admin_url, ca)
            .await
            .map_err(|_| Error::Trust)?;
        client.batch_execute("SET statement_timeout='30s'; SET lock_timeout='5s'; SET idle_in_transaction_session_timeout='30s'; SET standard_conforming_strings=on").await.map_err(|_| Error::Unavailable)?;
        Ok(Self {
            client: tokio::sync::Mutex::new(client),
            tls: ca.is_some(),
        })
    }
    /// Inspect retained ownership only. Unlike initial provisioning, missing markers are not repaired.
    pub async fn retained(&self, plan: &StoragePlan) -> Result<ProvisionedStorage> {
        let url = plan.url()?;
        if !self.tls {
            webdock_plugin_kit::loopback(&url).map_err(|_| Error::Trust)?;
        }
        let client = self.client.lock().await;
        let row = client.query_opt(
            "SELECT r.rolsuper,r.rolcreatedb,r.rolcreaterole,r.rolinherit,r.rolreplication,r.rolbypassrls,r.rolcanlogin,pg_catalog.shobj_description(r.oid,'pg_authid'),pg_catalog.shobj_description(d.oid,'pg_database'),EXISTS(SELECT FROM pg_catalog.pg_auth_members m WHERE m.member=r.oid) FROM pg_catalog.pg_database d JOIN pg_catalog.pg_roles r ON r.oid=d.datdba WHERE r.rolname=$1 AND d.datname=$2",
            &[&plan.role, &plan.database],
        ).await.map_err(|_| Error::Unavailable)?.ok_or(Error::Ownership)?;
        if (0..6).any(|i| row.get::<_, bool>(i))
            || !row.get::<_, bool>(6)
            || row.get::<_, Option<String>>(7).as_deref() != Some(plan.marker.as_str())
            || row.get::<_, Option<String>>(8).as_deref() != Some(plan.marker.as_str())
            || row.get::<_, bool>(9)
        {
            return Err(Error::Ownership);
        }
        Ok(ProvisionedStorage {
            url,
            database: plan.database.clone(),
            role: plan.role.clone(),
        })
    }
    pub async fn provision(&self, plan: &StoragePlan) -> Result<ProvisionedStorage> {
        let url = plan.url()?;
        if !self.tls {
            webdock_plugin_kit::loopback(&url).map_err(|_| Error::Trust)?;
        }
        let mut client = self.client.lock().await;
        client.execute("SELECT pg_catalog.pg_advisory_lock(pg_catalog.hashtext('webdock-plugin-metadata-provisioning'))", &[]).await.map_err(|_| Error::Unavailable)?;
        let result = self.provision_locked(&mut client, plan).await;
        let unlocked = client.execute("SELECT pg_catalog.pg_advisory_unlock(pg_catalog.hashtext('webdock-plugin-metadata-provisioning'))", &[]).await;
        result?;
        unlocked.map_err(|_| Error::Unavailable)?;
        Ok(ProvisionedStorage {
            url,
            database: plan.database.clone(),
            role: plan.role.clone(),
        })
    }
    async fn provision_locked(&self, client: &mut Client, plan: &StoragePlan) -> Result<()> {
        let tx = client.transaction().await.map_err(|_| Error::Unavailable)?;
        let role = tx.query_opt("SELECT r.rolsuper,r.rolcreatedb,r.rolcreaterole,r.rolinherit,r.rolreplication,r.rolbypassrls,r.rolcanlogin,pg_catalog.shobj_description(r.oid,'pg_authid'),EXISTS(SELECT FROM pg_catalog.pg_auth_members m WHERE m.member=r.oid) FROM pg_catalog.pg_roles r WHERE r.rolname=$1", &[&plan.role]).await.map_err(|_| Error::Unavailable)?;
        if let Some(row) = role {
            if (0..6).any(|i| row.get::<_, bool>(i))
                || !row.get::<_, bool>(6)
                || row.get::<_, Option<String>>(7).as_deref() != Some(plan.marker.as_str())
                || row.get::<_, bool>(8)
            {
                return Err(Error::Ownership);
            }
        } else {
            // Role and ownership marker commit together; a crash cannot leave an adoptable role.
            tx.batch_execute(&format!("CREATE ROLE {} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS PASSWORD {}; COMMENT ON ROLE {} IS {}", ident(&plan.role), literal(&plan.password), ident(&plan.role), literal(&plan.marker))).await.map_err(|_| Error::Unavailable)?;
        }
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        let database = client.query_opt("SELECT pg_catalog.pg_get_userbyid(datdba),pg_catalog.shobj_description(oid,'pg_database') FROM pg_catalog.pg_database WHERE datname=$1", &[&plan.database]).await.map_err(|_| Error::Unavailable)?;
        if let Some(row) = database {
            let marker: Option<String> = row.get(1);
            if row.get::<_, String>(0) != plan.role
                || marker.as_ref().is_some_and(|v| v != &plan.marker)
            {
                return Err(Error::Ownership);
            }
            // Missing database comment is the crash boundary after CREATE DATABASE. Its owner is
            // the atomically marked NOCREATEDB role, which cannot have created a foreign database.
        } else {
            client
                .batch_execute(&format!(
                    "CREATE DATABASE {} OWNER {} TEMPLATE template0",
                    ident(&plan.database),
                    ident(&plan.role)
                ))
                .await
                .map_err(|_| Error::Unavailable)?;
        }
        client
            .batch_execute(&format!(
                "COMMENT ON DATABASE {} IS {}",
                ident(&plan.database),
                literal(&plan.marker)
            ))
            .await
            .map_err(|_| Error::Unavailable)?;
        // This service is exclusively for plugin metadata. PUBLIC must not grant our new login
        // CONNECT to another plugin's or the maintenance database, including future template use.
        let databases = client
            .query(
                "SELECT datname FROM pg_catalog.pg_database ORDER BY datname",
                &[],
            )
            .await
            .map_err(|_| Error::Unavailable)?;
        for row in databases {
            let name: String = row.get(0);
            client
                .batch_execute(&format!(
                    "REVOKE CONNECT,TEMPORARY ON DATABASE {} FROM PUBLIC",
                    ident(&name)
                ))
                .await
                .map_err(|_| Error::Unavailable)?;
            if name != plan.database {
                client
                    .batch_execute(&format!(
                        "REVOKE ALL PRIVILEGES ON DATABASE {} FROM {}",
                        ident(&name),
                        ident(&plan.role)
                    ))
                    .await
                    .map_err(|_| Error::Unavailable)?;
            }
        }
        client
            .batch_execute(&format!(
                "GRANT CONNECT,TEMPORARY,CREATE ON DATABASE {} TO {}",
                ident(&plan.database),
                ident(&plan.role)
            ))
            .await
            .map_err(|_| Error::Unavailable)?;
        Ok(())
    }
}
