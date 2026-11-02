//! Private persisted authority codec only. No authentication, grant or lease behavior.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use webdock_browser_session::StoredAuthority;
use webdock_oauth::McpAuthority;
pub enum DatabaseAuthority {
    BrowserStored(StoredAuthority),
    Mcp(Box<McpAuthority>),
}
impl DatabaseAuthority {
    pub fn from_persisted(value: Value) -> Result<Self, &'static str> {
        if value.is_null() {
            return Ok(Self::BrowserStored(StoredAuthority::LocalSession {}));
        }
        if value["kind"] == "database-mcp" {
            let stored: StoredMcp =
                serde_json::from_value(value).map_err(|_| "Invalid private database authority")?;
            if stored.version != 1 {
                return Err("Unsupported private database authority version");
            }
            return Ok(Self::Mcp(stored.reference));
        }
        serde_json::from_value(value)
            .map(Self::BrowserStored)
            .map_err(|_| "Invalid retained browser authority")
    }
    pub fn to_persisted(&self) -> Result<Value, &'static str> {
        match self {
            Self::BrowserStored(browser) => {
                serde_json::to_value(browser).map_err(|_| "Invalid retained browser authority")
            }
            Self::Mcp(reference) => {
                Ok(json!({"kind":"database-mcp","version":1,"reference":reference}))
            }
        }
    }
    pub fn read_only(&self) -> bool {
        match self {
            Self::BrowserStored(browser) => browser.read_only(),
            Self::Mcp(reference) => !reference.can_write(),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredMcp {
    kind: McpKind,
    version: u8,
    reference: Box<McpAuthority>,
}
#[derive(Serialize, Deserialize)]
enum McpKind {
    #[serde(rename = "database-mcp")]
    DatabaseMcp,
}
