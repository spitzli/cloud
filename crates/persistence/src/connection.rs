//! Explicit connection authority shared by runtimes and offline commands.
//! SQLx accepts query parameters that override URL authority; reviewed targets must not drift.
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sqlx::postgres::PgSslMode;
use std::str::FromStr;

#[derive(Debug, thiserror::Error)]
#[error("Invalid PostgreSQL connection policy")]
pub struct ConnectionPolicyError;

/// Contains credentials; deliberately not Debug or Serialize.
pub struct ConnectionUrl {
    raw: String,
    target: String,
}

/// Verify the authenticated backend role as well as mutable current/session authorization.
/// Qualified catalog relations cannot be shadowed by a temporary table or altered search path.
pub async fn verify_runtime_role(
    db: &impl ConnectionTrait,
    expected: &str,
) -> Result<(), ConnectionPolicyError> {
    if !crate::roles::LOGIN_ROLES.contains(&expected) {
        return Err(ConnectionPolicyError);
    }
    let row = db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT current_user::text=$1 AND session_user::text=$1 AND b.usename::text=$1
                AND NOT (r.rolsuper OR r.rolbypassrls OR r.rolcreatedb OR r.rolcreaterole OR r.rolreplication)
                AND NOT pg_catalog.pg_has_role(current_user,'webdock_platform_owner','MEMBER')
                AND NOT pg_catalog.pg_has_role(current_user,'webdock_policy_owner','MEMBER')
                AND NOT pg_catalog.pg_has_role(current_user,'webdock_website_owner','MEMBER') AS restricted
         FROM pg_catalog.pg_roles r
         JOIN pg_catalog.pg_stat_activity b ON b.pid=pg_catalog.pg_backend_pid()
         WHERE r.rolname=current_user",
        [expected.into()],
    )).await.map_err(|_| ConnectionPolicyError)?.ok_or(ConnectionPolicyError)?;
    if !row
        .try_get::<bool>("", "restricted")
        .map_err(|_| ConnectionPolicyError)?
    {
        return Err(ConnectionPolicyError);
    }
    Ok(())
}
impl ConnectionUrl {
    pub fn parse(raw: &str, require_tls: bool) -> Result<Self, ConnectionPolicyError> {
        let mut url = url::Url::parse(raw).map_err(|_| ConnectionPolicyError)?;
        if !matches!(url.scheme(), "postgres" | "postgresql")
            || url.username().is_empty()
            || url.fragment().is_some()
        {
            return Err(ConnectionPolicyError);
        }
        let local = match url.host().ok_or(ConnectionPolicyError)? {
            url::Host::Domain("localhost") => true,
            url::Host::Domain(host) if !host.contains('%') => host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback()),
            url::Host::Ipv4(ip) => ip.is_loopback(),
            url::Host::Ipv6(ip) => ip.is_loopback(),
            url::Host::Domain(_) => return Err(ConnectionPolicyError),
        };
        let database = url.path().strip_prefix('/').ok_or(ConnectionPolicyError)?;
        if database.is_empty()
            || !database
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            return Err(ConnectionPolicyError);
        }
        let mut mode = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "host" | "hostaddr" | "port" | "dbname" | "user" | "password" => {
                    return Err(ConnectionPolicyError);
                }
                "sslmode" | "ssl-mode" => {
                    if mode.is_some() {
                        return Err(ConnectionPolicyError);
                    }
                    mode = Some(PgSslMode::from_str(&value).map_err(|_| ConnectionPolicyError)?);
                }
                "sslrootcert"
                | "ssl-root-cert"
                | "ssl-ca"
                | "sslcert"
                | "ssl-cert"
                | "sslkey"
                | "ssl-key"
                | "statement-cache-capacity"
                | "application_name"
                | "options" => {}
                key if key.starts_with("options[") && key.ends_with(']') => {}
                // The driver logs unknown parameter values; reject them before parsing/connecting.
                _ => return Err(ConnectionPolicyError),
            }
        }
        if (require_tls || !local) && !matches!(mode, Some(PgSslMode::VerifyFull)) {
            return Err(ConnectionPolicyError);
        }
        let target = format!(
            "{}:{}{}",
            url.host_str().ok_or(ConnectionPolicyError)?,
            url.port().unwrap_or(5432),
            url.path(),
        );
        // SQLx otherwise lets PGPORT/PGSSLMODE change an omitted URL value.
        url.set_port(Some(url.port().unwrap_or(5432)))
            .map_err(|_| ConnectionPolicyError)?;
        if mode.is_none() {
            url.query_pairs_mut().append_pair("sslmode", "prefer");
        }
        Ok(Self {
            raw: url.into(),
            target,
        })
    }
    pub fn as_str(&self) -> &str {
        &self.raw
    }
    pub fn require_target(&self, target: &str) -> Result<(), ConnectionPolicyError> {
        if target != self.target {
            return Err(ConnectionPolicyError);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgConnectOptions;

    #[test]
    fn rejects_tls_alias_downgrades_and_target_overrides() {
        for query in [
            "sslmode=verify-full&sslmode=disable",
            "sslmode=verify-full&ssl-mode=disable",
            "ssl-mode=disable&sslmode=verify-full",
            "sslmode=verify-full&host=elsewhere.invalid",
            "sslmode=verify-full&hostaddr=192.0.2.1",
            "sslmode=verify-full&port=5433",
            "sslmode=verify-full&dbname=other",
            "sslmode=verify-full&user=owner",
            "sslmode=verify-full&password=override",
        ] {
            assert!(
                ConnectionUrl::parse(
                    &format!("postgres://runtime:test@localhost/db?{query}"),
                    false
                )
                .is_err()
            );
        }
        assert!(ConnectionUrl::parse("postgres://runtime:test@db.invalid/db", false).is_err());
        assert!(
            ConnectionUrl::parse(
                "postgres://runtime:test@db.invalid/db?sslmode=require",
                false
            )
            .is_err()
        );
        assert!(
            ConnectionUrl::parse("postgres://runtime:test@localhost/db?sslmode=prefer", true)
                .is_err()
        );
    }

    #[test]
    fn explicit_canonical_target_and_typed_driver_tls_agree() {
        for host in ["localhost", "127.0.0.1", "[::1]"] {
            ConnectionUrl::parse(&format!("postgres://runtime:test@{host}/db"), false).unwrap();
        }
        let url = ConnectionUrl::parse("postgres://runtime:test@localhost/db", false).unwrap();
        url.require_target("localhost:5432/db").unwrap();
        assert!(url.require_target("localhost:5433/db").is_err());
        let options = PgConnectOptions::from_str(url.as_str()).unwrap();
        assert_eq!(options.get_port(), 5432);
        assert_eq!(options.get_host(), "localhost");
        assert_eq!(options.get_database(), Some("db"));
        assert!(matches!(options.get_ssl_mode(), PgSslMode::Prefer));
        let remote = ConnectionUrl::parse(
            "postgres://runtime:test@db.invalid:6543/db?ssl-mode=verify-full",
            true,
        )
        .unwrap();
        remote.require_target("db.invalid:6543/db").unwrap();
        assert!(matches!(
            PgConnectOptions::from_str(remote.as_str())
                .unwrap()
                .get_ssl_mode(),
            PgSslMode::VerifyFull
        ));
    }

    #[test]
    fn credentials_and_ambiguous_authority_never_appear_in_errors() {
        for raw in [
            "postgres://runtime:private@localhost/db?host=elsewhere.invalid",
            "postgres://runtime:private@%2Ftmp/db",
            "postgres://runtime:private@localhost/",
            "postgres://runtime:private@localhost/db#ignored",
            "postgres://localhost/db",
            "postgres://runtime:private@localhost/one/two",
            "postgres://runtime:private@localhost/db?unknown_secret=private",
            "https://runtime:private@localhost/db",
        ] {
            let error = ConnectionUrl::parse(raw, false)
                .err()
                .expect("invalid authority");
            assert_eq!(error.to_string(), "Invalid PostgreSQL connection policy");
            assert!(!format!("{error:?}").contains("private"));
        }
    }
}
