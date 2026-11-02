use loco_rs::config::Config;
use serde_json::json;
use std::collections::HashMap;
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

// This startup adapter intentionally returns the framework's externally defined error.
#[expect(
    clippy::result_large_err,
    reason = "The Loco framework requires its error type at this startup boundary."
)]
pub fn runtime_config(env: &HashMap<String, String>) -> loco_rs::Result<Config> {
    let platform =
        PlatformConfig::from_map(env).map_err(|e| loco_rs::Error::Message(e.to_string()))?;
    let database_url = env
        .get("WEBDOCK_DATABASE_URL")
        .ok_or_else(|| loco_rs::Error::Message("Missing WEBDOCK_DATABASE_URL".into()))?;
    let database_url =
        webdock_persistence::connection::ConnectionUrl::parse(database_url, platform.production)
            .map_err(|_| loco_rs::Error::Message("Invalid PostgreSQL connection policy".into()))?;
    serde_json::from_value(json!({
        "logger": {"enable":true,"level":"info","format":"json"},
        "server": {"port":platform.port,"binding":env.get("WEBDOCK_BIND_ADDRESS").map(String::as_str).unwrap_or("127.0.0.1"),"host":platform.api_origin,"middlewares":{}},
        "database": {"uri":database_url.as_str(),"enable_logging":false,"min_connections":1,"max_connections":10,"connect_timeout":5000,"idle_timeout":30000,"acquire_timeout":5000,"auto_migrate":false,"dangerously_truncate":false,"dangerously_recreate":false},
        "workers":{"mode":"ForegroundBlocking"},
        "queue":null,"auth":null,"mailer":null,"initializers":null,"scheduler":null,
        "settings":{"platform":"webdock"}
    })).map_err(Into::into)
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
