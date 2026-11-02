use crate::ContractError;
use crate::features::FeatureSet;
use std::collections::HashMap;
use url::Url;

#[derive(Debug, Clone)]
pub struct PlatformConfig {
    pub api_origin: String,
    pub auth_origin: String,
    pub studio_origin: String,
    pub console_origin: Option<String>,
    pub port: u16,
    pub production: bool,
    pub features: FeatureSet,
    pub database_new_orders_enabled: bool,
}
impl PlatformConfig {
    pub fn from_map(env: &HashMap<String, String>) -> Result<Self, ContractError> {
        let production = env
            .get("NODE_ENV")
            .is_some_and(|value| value == "production")
            || env
                .get("LOCO_ENV")
                .is_some_and(|value| value == "production");
        let read_origin = |key: &str, fallback: &str| -> Result<String, ContractError> {
            if production && !env.contains_key(key) {
                return Err(ContractError(format!("Missing {key}")));
            }
            canonical_origin(
                env.get(key).map(String::as_str).unwrap_or(fallback),
                production,
            )
        };
        let api_origin = read_origin("WEBDOCK_API_ORIGIN", "http://localhost:3130")?;
        let auth_key = if env.contains_key("WEBDOCK_AUTH_ORIGIN") {
            "WEBDOCK_AUTH_ORIGIN"
        } else {
            "BETTER_AUTH_URL"
        };
        let auth_origin = read_origin(auth_key, "http://localhost:3125")?;
        let studio_origin = read_origin("WEBDOCK_STUDIO_ORIGIN", "http://localhost:3120")?;
        let console_origin = env
            .get("WEBDOCK_CONSOLE_ORIGIN")
            .map(|value| canonical_origin(value, production))
            .transpose()?;
        let mut origins = vec![&api_origin, &auth_origin, &studio_origin];
        if let Some(origin) = &console_origin {
            origins.push(origin);
        }
        origins.sort_unstable();
        let original_len = origins.len();
        origins.dedup();
        if origins.len() != original_len {
            return Err(ContractError("Canonical origins must be distinct".into()));
        }
        let raw_port = env.get("PORT").map(String::as_str).unwrap_or("3130");
        let port = raw_port
            .parse::<u16>()
            .ok()
            .filter(|value| *value > 0)
            .filter(|_| raw_port.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or_else(|| ContractError("Invalid port".into()))?;
        let features = FeatureSet::from_json(
            env.get("WEBDOCK_FEATURES")
                .map(String::as_str)
                .unwrap_or("{}"),
        )?;
        let database_new_orders_enabled = match env
            .get("WEBDOCK_DATABASE_NEW_ORDERS_ENABLED")
            .map(String::as_str)
        {
            None | Some("false") => false,
            Some("true") => true,
            Some(_) => {
                return Err(ContractError(
                    "Invalid WEBDOCK_DATABASE_NEW_ORDERS_ENABLED".into(),
                ));
            }
        };
        Ok(Self {
            api_origin,
            auth_origin,
            studio_origin,
            console_origin,
            port,
            production,
            features,
            database_new_orders_enabled,
        })
    }

    pub fn console_origin(&self) -> &str {
        self.console_origin
            .as_deref()
            .unwrap_or(&self.studio_origin)
    }
    pub fn allowed_origins(&self) -> Vec<&str> {
        let mut origins = vec![
            self.api_origin.as_str(),
            self.auth_origin.as_str(),
            self.studio_origin.as_str(),
        ];
        if let Some(origin) = &self.console_origin {
            origins.push(origin);
        }
        origins
    }
}

pub fn canonical_origin(raw: &str, production: bool) -> Result<String, ContractError> {
    let invalid = || ContractError("Invalid canonical origin".into());
    if raw.trim() != raw || raw.contains('\\') {
        return Err(invalid());
    }
    let url = Url::parse(raw).map_err(|_| invalid())?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https" || (!production && loopback && url.scheme() == "http"))
    {
        return Err(invalid());
    }
    Ok(url.origin().ascii_serialization())
}
