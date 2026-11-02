use crate::{Result, invalid};
use serde::Deserialize;
use serde_json::Value;
pub use webdock_browser_session::StoredAuthority;
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum Command {
    #[serde(rename_all = "camelCase")]
    List {
        #[serde(rename = "customerID")]
        customer_id: String,
    },
    Get {
        #[serde(rename = "bindingID")]
        binding_id: String,
    },
    Create {
        #[serde(rename = "projectID")]
        project_id: String,
        name: String,
        environment: String,
        #[serde(default = "engine")]
        engine: String,
    },
    Grant {
        #[serde(rename = "bindingID")]
        binding_id: String,
        subject: String,
        profile: String,
        #[serde(rename = "runtimeOrigin")]
        runtime_origin: String,
        #[serde(rename = "connectionID")]
        connection_id: String,
        #[serde(rename = "proxySecret")]
        proxy_secret: String,
        #[serde(rename = "isolationVerified")]
        isolation_verified: bool,
        #[serde(rename = "databaseRoleVerified")]
        database_role_verified: bool,
    },
    Revoke {
        #[serde(rename = "bindingID")]
        binding_id: String,
        subject: String,
    },
    Disable {
        #[serde(rename = "bindingID")]
        binding_id: String,
    },
    Open {
        #[serde(rename = "bindingID")]
        binding_id: String,
    },
    OpenManaged {
        #[serde(rename = "databaseID")]
        database_id: String,
    },
}
fn engine() -> String {
    "postgresql".into()
}
pub fn normalize_origin(raw: &str) -> Result<String> {
    if raw.len() > 300 || raw.bytes().any(|b| b <= 32 || b == 127 || b == b'\\') {
        return Err(invalid());
    }
    let u = url::Url::parse(raw).map_err(|_| invalid())?;
    let local = u.scheme() == "http"
        && [Some("localhost"), Some("127.0.0.1"), Some("[::1]")].contains(&u.host_str());
    if (!local && u.scheme() != "https")
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.path() != "/"
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(u.origin().ascii_serialization())
}
pub(crate) fn id(s: &str) -> Result<()> {
    if s.is_empty()
        || s.starts_with('0')
        || !s.bytes().all(|b| b.is_ascii_digit())
        || s.parse::<i64>().is_err()
    {
        Err(invalid())
    } else {
        Ok(())
    }
}
fn bounded(s: &str, min: usize, max: usize) -> Result<()> {
    if s.chars().count() < min || s.chars().count() > max || s.chars().any(char::is_control) {
        Err(invalid())
    } else {
        Ok(())
    }
}
impl Command {
    pub fn parse(value: Value) -> Result<Self> {
        let mut cmd: Self = serde_json::from_value(value).map_err(|_| invalid())?;
        match &mut cmd {
            Self::List { customer_id } => id(customer_id)?,
            Self::Create {
                project_id,
                name,
                environment,
                engine,
            } => {
                id(project_id)?;
                *name = name.trim().into();
                bounded(name, 1, 120)?;
                if !["production", "staging", "development"].contains(&environment.as_str())
                    || !["postgresql", "sqlite"].contains(&engine.as_str())
                {
                    return Err(invalid());
                }
            }
            Self::Grant {
                binding_id,
                subject,
                profile,
                runtime_origin,
                connection_id,
                proxy_secret,
                isolation_verified,
                database_role_verified,
            } => {
                id(binding_id)?;
                bounded(subject, 1, 128)?;
                bounded(connection_id, 1, 128)?;
                bounded(proxy_secret, 32, 512)?;
                *runtime_origin = normalize_origin(runtime_origin)?;
                if !*isolation_verified
                    || !*database_role_verified
                    || !["read", "write", "schema"].contains(&profile.as_str())
                {
                    return Err(invalid());
                }
            }
            Self::Revoke {
                binding_id,
                subject,
            } => {
                id(binding_id)?;
                bounded(subject, 1, 128)?
            }
            Self::Get { binding_id } | Self::Disable { binding_id } | Self::Open { binding_id } => {
                id(binding_id)?
            }
            Self::OpenManaged { database_id } => id(database_id)?,
        }
        Ok(cmd)
    }
    pub fn mutates(&self) -> bool {
        !matches!(self, Self::List { .. } | Self::Get { .. })
    }
}
