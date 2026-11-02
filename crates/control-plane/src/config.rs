use loco_rs::config::Config;
use serde_json::json;
use std::collections::HashMap;
use webdock_core::config::PlatformConfig;

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
