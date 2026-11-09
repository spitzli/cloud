use crate::runtime::{Error, Result as RuntimeResult};
use std::collections::HashMap;
use std::time::Duration;
use webdock_core::config::PlatformConfig;

/// Retained protocol identifiers are independent of the public account origin.
pub(crate) struct AuthCompatibility {
    pub passkey_rp_origin: url::Url,
    pub oauth_issuer: String,
}
impl AuthCompatibility {
    pub fn from_map(
        env: &HashMap<String, String>,
        platform: &PlatformConfig,
    ) -> Result<Self, webdock_core::ContractError> {
        let origin = |key: &str| {
            webdock_core::config::canonical_origin(
                env.get(key)
                    .map(String::as_str)
                    .unwrap_or(&platform.auth_origin),
                platform.production,
            )
        };
        let passkey_rp_origin = url::Url::parse(&origin("WEBDOCK_PASSKEY_RP_ORIGIN")?)
            .map_err(|_| webdock_core::ContractError("Invalid passkey RP origin".into()))?;
        let oauth_issuer = format!("{}/api/auth", origin("WEBDOCK_OAUTH_ISSUER_ORIGIN")?);
        Ok(Self {
            passkey_rp_origin,
            oauth_issuer,
        })
    }
}

/// Startup-only secret/configuration reader: owned regular file, no symlink following,
/// descriptor-based permission check and a bounded read even if the file grows.
pub fn read_private_file(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    use std::{
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let denied = || {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Owned private configuration file required",
        )
    };
    if !path.is_absolute() {
        return Err(denied());
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || metadata.len() > 1024 * 1024
    {
        return Err(denied());
    }
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err(denied());
    }
    Ok(bytes)
}

/// Contains credentials and deliberately implements neither Debug nor Serialize.
pub struct RuntimeDatabaseConfig {
    pub uri: webdock_persistence::connection::ConnectionUrl,
    pub min_connections: u32,
    pub max_connections: u32,
    pub connect_timeout: Duration,
    pub acquire_timeout: Duration,
    pub idle_timeout: Duration,
}
pub struct RuntimeServerConfig {
    pub binding: String,
    pub port: u16,
    pub shutdown_timeout: Duration,
}
#[derive(Clone, Copy)]
pub enum IdBackend {
    Legacy,
    Native,
}
#[derive(Clone, Copy)]
pub enum SessionReadBackend {
    Legacy,
    Native,
}
#[derive(Clone, Copy)]
pub enum SessionManagementBackend {
    Legacy,
    Native,
}
#[derive(Clone, Copy)]
pub enum BrowserFlowBackend {
    Legacy,
    Native,
}
#[derive(Clone, Copy)]
pub enum OAuthClientBackend {
    Legacy,
    Native,
}
#[derive(Clone, Copy)]
pub enum OAuthIntrospectionBackend {
    Legacy,
    Native,
}
#[derive(Clone, Copy)]
pub enum BrowserAuthorityBackend {
    Legacy,
    Native,
}
#[derive(Clone, Copy)]
pub enum OAuthSecurityBackend {
    Legacy,
    Native,
}
#[derive(Clone, Copy)]
pub enum OAuthGrantsBackend {
    Legacy,
    Native,
}
#[derive(Clone, Copy)]
pub enum OAuthTokensBackend {
    Legacy,
    Native,
}
#[derive(Clone, Copy)]
pub enum McpAuthorityBackend {
    Legacy,
    Native,
}
#[derive(Clone, Copy)]
pub enum AccountRecoveryBackend {
    Legacy,
    Native,
}
pub struct RuntimeConfig {
    pub account_recovery_backend: AccountRecoveryBackend,
    pub mcp_authority_backend: McpAuthorityBackend,
    pub oauth_tokens_backend: OAuthTokensBackend,
    pub oauth_grants_backend: OAuthGrantsBackend,
    pub oauth_security_backend: OAuthSecurityBackend,
    pub browser_authority_backend: BrowserAuthorityBackend,
    pub oauth_introspection_backend: OAuthIntrospectionBackend,
    pub oauth_client_backend: OAuthClientBackend,
    pub browser_flow_backend: BrowserFlowBackend,
    pub session_management_backend: SessionManagementBackend,
    pub session_read_backend: SessionReadBackend,
    pub id_backend: IdBackend,
    pub database: RuntimeDatabaseConfig,
    pub server: RuntimeServerConfig,
    pub platform: PlatformConfig,
}

pub fn runtime_config(env: &HashMap<String, String>) -> RuntimeResult<RuntimeConfig> {
    let platform = PlatformConfig::from_map(env).map_err(|e| Error::Message(e.to_string()))?;
    let database_url = env
        .get("WEBDOCK_DATABASE_URL")
        .ok_or_else(|| Error::Message("Missing WEBDOCK_DATABASE_URL".into()))?;
    let uri =
        webdock_persistence::connection::ConnectionUrl::parse(database_url, platform.production)
            .map_err(|_| Error::Message("Invalid PostgreSQL connection policy".into()))?;
    let binding = env
        .get("WEBDOCK_BIND_ADDRESS")
        .map(String::as_str)
        .unwrap_or("127.0.0.1");
    if binding.is_empty() || binding.chars().any(char::is_whitespace) {
        return Err(Error::Message("Invalid HTTP binding".into()));
    }
    let binding = if let Some(host) = binding
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
    {
        host.parse::<std::net::Ipv6Addr>()
            .map_err(|_| Error::Message("Invalid IPv6 binding".into()))?
            .to_string()
    } else {
        binding.to_owned()
    };
    let id_backend = match env.get("WEBDOCK_ID_BACKEND").map(String::as_str) {
        None | Some("sea-orm") => IdBackend::Legacy,
        Some("tokio-postgres") => IdBackend::Native,
        _ => return Err(Error::Message("Invalid WEBDOCK_ID_BACKEND".into())),
    };
    let session_read_backend = match env.get("WEBDOCK_SESSION_READ_BACKEND").map(String::as_str) {
        None | Some("sea-orm") => SessionReadBackend::Legacy,
        Some("tokio-postgres") => SessionReadBackend::Native,
        _ => {
            return Err(Error::Message(
                "Invalid WEBDOCK_SESSION_READ_BACKEND".into(),
            ));
        }
    };
    let session_management_backend = match env
        .get("WEBDOCK_SESSION_MANAGEMENT_BACKEND")
        .map(String::as_str)
    {
        None | Some("sea-orm") => SessionManagementBackend::Legacy,
        Some("tokio-postgres") => SessionManagementBackend::Native,
        _ => {
            return Err(Error::Message(
                "Invalid WEBDOCK_SESSION_MANAGEMENT_BACKEND".into(),
            ));
        }
    };
    let session_read_backend =
        if matches!(session_management_backend, SessionManagementBackend::Native) {
            SessionReadBackend::Native
        } else {
            session_read_backend
        };
    let browser_flow_backend = match env.get("WEBDOCK_BROWSER_FLOW_BACKEND").map(String::as_str) {
        None | Some("sea-orm") => BrowserFlowBackend::Legacy,
        Some("tokio-postgres") => BrowserFlowBackend::Native,
        _ => {
            return Err(Error::Message(
                "Invalid WEBDOCK_BROWSER_FLOW_BACKEND".into(),
            ));
        }
    };
    let oauth_client_backend = match env.get("WEBDOCK_OAUTH_CLIENT_BACKEND").map(String::as_str) {
        None | Some("sea-orm") => OAuthClientBackend::Legacy,
        Some("tokio-postgres") => OAuthClientBackend::Native,
        _ => {
            return Err(Error::Message(
                "Invalid WEBDOCK_OAUTH_CLIENT_BACKEND".into(),
            ));
        }
    };
    let oauth_introspection_backend = match env
        .get("WEBDOCK_OAUTH_INTROSPECTION_BACKEND")
        .map(String::as_str)
    {
        None | Some("sea-orm") => OAuthIntrospectionBackend::Legacy,
        Some("tokio-postgres") => OAuthIntrospectionBackend::Native,
        _ => {
            return Err(Error::Message(
                "Invalid WEBDOCK_OAUTH_INTROSPECTION_BACKEND".into(),
            ));
        }
    };
    let oauth_security_backend = match env
        .get("WEBDOCK_OAUTH_SECURITY_BACKEND")
        .map(String::as_str)
    {
        None | Some("sea-orm") => OAuthSecurityBackend::Legacy,
        Some("tokio-postgres") => OAuthSecurityBackend::Native,
        _ => {
            return Err(Error::Message(
                "Invalid WEBDOCK_OAUTH_SECURITY_BACKEND".into(),
            ));
        }
    };
    let oauth_grants_backend = match env.get("WEBDOCK_OAUTH_GRANTS_BACKEND").map(String::as_str) {
        None | Some("sea-orm") => OAuthGrantsBackend::Legacy,
        Some("tokio-postgres") => OAuthGrantsBackend::Native,
        _ => {
            return Err(Error::Message(
                "Invalid WEBDOCK_OAUTH_GRANTS_BACKEND".into(),
            ));
        }
    };
    let oauth_tokens_backend = match env.get("WEBDOCK_OAUTH_TOKENS_BACKEND").map(String::as_str) {
        None | Some("sea-orm") => OAuthTokensBackend::Legacy,
        Some("tokio-postgres") => OAuthTokensBackend::Native,
        _ => {
            return Err(Error::Message(
                "Invalid WEBDOCK_OAUTH_TOKENS_BACKEND".into(),
            ));
        }
    };
    let oauth_grants_backend = if matches!(oauth_tokens_backend, OAuthTokensBackend::Native) {
        OAuthGrantsBackend::Native
    } else {
        oauth_grants_backend
    };
    let oauth_security_backend = if matches!(oauth_grants_backend, OAuthGrantsBackend::Native) {
        OAuthSecurityBackend::Native
    } else {
        oauth_security_backend
    };
    let id_backend = if matches!(oauth_security_backend, OAuthSecurityBackend::Native) {
        IdBackend::Native
    } else {
        id_backend
    };
    let session_read_backend = if matches!(oauth_security_backend, OAuthSecurityBackend::Native) {
        SessionReadBackend::Native
    } else {
        session_read_backend
    };
    let oauth_introspection_backend =
        if matches!(oauth_security_backend, OAuthSecurityBackend::Native) {
            OAuthIntrospectionBackend::Native
        } else {
            oauth_introspection_backend
        };
    let mcp_authority_backend = match env.get("WEBDOCK_MCP_AUTHORITY_BACKEND").map(String::as_str) {
        None | Some("sea-orm") => McpAuthorityBackend::Legacy,
        Some("tokio-postgres") => McpAuthorityBackend::Native,
        _ => {
            return Err(Error::Message(
                "Invalid WEBDOCK_MCP_AUTHORITY_BACKEND".into(),
            ));
        }
    };
    let oauth_introspection_backend =
        if matches!(mcp_authority_backend, McpAuthorityBackend::Native) {
            OAuthIntrospectionBackend::Native
        } else {
            oauth_introspection_backend
        };
    let browser_authority_backend = match env
        .get("WEBDOCK_BROWSER_AUTHORITY_BACKEND")
        .map(String::as_str)
    {
        None | Some("sea-orm") => BrowserAuthorityBackend::Legacy,
        Some("tokio-postgres") => BrowserAuthorityBackend::Native,
        _ => {
            return Err(Error::Message(
                "Invalid WEBDOCK_BROWSER_AUTHORITY_BACKEND".into(),
            ));
        }
    };
    let browser_flow_backend =
        if matches!(browser_authority_backend, BrowserAuthorityBackend::Native) {
            BrowserFlowBackend::Native
        } else {
            browser_flow_backend
        };
    let oauth_introspection_backend =
        if matches!(browser_authority_backend, BrowserAuthorityBackend::Native) {
            OAuthIntrospectionBackend::Native
        } else {
            oauth_introspection_backend
        };
    let oauth_client_backend = if matches!(
        oauth_introspection_backend,
        OAuthIntrospectionBackend::Native
    ) {
        OAuthClientBackend::Native
    } else {
        oauth_client_backend
    };
    let account_recovery_backend = match env
        .get("WEBDOCK_ACCOUNT_RECOVERY_BACKEND")
        .map(String::as_str)
    {
        None | Some("sea-orm") => AccountRecoveryBackend::Legacy,
        Some("tokio-postgres") => AccountRecoveryBackend::Native,
        _ => {
            return Err(Error::Message(
                "Invalid WEBDOCK_ACCOUNT_RECOVERY_BACKEND".into(),
            ));
        }
    };
    let id_backend = if matches!(account_recovery_backend, AccountRecoveryBackend::Native) {
        IdBackend::Native
    } else {
        id_backend
    };
    Ok(RuntimeConfig {
        account_recovery_backend,
        mcp_authority_backend,
        oauth_tokens_backend,
        oauth_grants_backend,
        oauth_security_backend,
        browser_authority_backend,
        oauth_introspection_backend,
        oauth_client_backend,
        browser_flow_backend,
        session_management_backend,
        session_read_backend,
        id_backend,
        database: RuntimeDatabaseConfig {
            uri,
            min_connections: 1,
            max_connections: 10,
            connect_timeout: Duration::from_secs(5),
            acquire_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(30),
        },
        server: RuntimeServerConfig {
            binding,
            port: platform.port,
            shutdown_timeout: Duration::from_secs(20),
        },
        platform,
    })
}

#[cfg(test)]
mod auth_compatibility {
    use super::*;

    fn production() -> (HashMap<String, String>, PlatformConfig) {
        let env = HashMap::from([
            ("NODE_ENV".into(), "production".into()),
            (
                "WEBDOCK_API_ORIGIN".into(),
                "https://api.spitzli.cloud".into(),
            ),
            (
                "WEBDOCK_AUTH_ORIGIN".into(),
                "https://auth.spitzli.cloud".into(),
            ),
            (
                "WEBDOCK_STUDIO_ORIGIN".into(),
                "https://studio.spitzli.cloud".into(),
            ),
        ]);
        let platform = PlatformConfig::from_map(&env).unwrap();
        (env, platform)
    }

    #[test]
    fn default_protocol_identifiers_follow_account_origin() {
        let (env, platform) = production();
        let compatibility = AuthCompatibility::from_map(&env, &platform).unwrap();
        assert_eq!(
            compatibility.passkey_rp_origin.as_str(),
            "https://auth.spitzli.cloud/"
        );
        assert_eq!(
            compatibility.oauth_issuer,
            "https://auth.spitzli.cloud/api/auth"
        );
    }

    #[test]
    fn local_defaults_keep_existing_development_behavior() {
        let env = HashMap::new();
        let platform = PlatformConfig::from_map(&env).unwrap();
        let compatibility = AuthCompatibility::from_map(&env, &platform).unwrap();
        assert_eq!(
            compatibility.passkey_rp_origin.as_str(),
            "http://localhost:3125/"
        );
        assert_eq!(compatibility.oauth_issuer, "http://localhost:3125/api/auth");
    }

    #[test]
    fn explicit_protocol_origins_preserve_identifiers_without_changing_account() {
        let (mut env, platform) = production();
        env.insert(
            "WEBDOCK_PASSKEY_RP_ORIGIN".into(),
            "https://auth.webdock.dev/".into(),
        );
        env.insert(
            "WEBDOCK_OAUTH_ISSUER_ORIGIN".into(),
            "https://auth.webdock.dev/".into(),
        );
        let compatibility = AuthCompatibility::from_map(&env, &platform).unwrap();
        assert_eq!(
            compatibility.passkey_rp_origin.as_str(),
            "https://auth.webdock.dev/"
        );
        assert_eq!(
            compatibility.oauth_issuer,
            "https://auth.webdock.dev/api/auth"
        );
        assert_eq!(platform.auth_origin, "https://auth.spitzli.cloud");
        assert!(
            !platform
                .allowed_origins()
                .contains(&"https://auth.webdock.dev")
        );
    }

    #[test]
    fn invalid_production_protocol_origins_are_rejected() {
        for key in ["WEBDOCK_PASSKEY_RP_ORIGIN", "WEBDOCK_OAUTH_ISSUER_ORIGIN"] {
            for invalid in [
                "http://auth.webdock.dev",
                "http://localhost:3125",
                "ftp://auth.webdock.dev",
                "https://user@auth.webdock.dev",
                "https://auth.webdock.dev/api/auth",
                "https://auth.webdock.dev?query=1",
                "https://auth.webdock.dev#fragment",
                " https://auth.webdock.dev",
                "https://auth.webdock.dev\\path",
                "",
            ] {
                let (mut env, platform) = production();
                env.insert(key.into(), invalid.into());
                assert!(
                    AuthCompatibility::from_map(&env, &platform).is_err(),
                    "{key}: {invalid}"
                );
            }
        }
    }
}
