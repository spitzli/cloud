//! Fixed bearer metadata surface. Browser command schemas and array responses stay unchanged.
use crate::{ManagedCommand, Result, invalid};
use serde_json::{Value, json};

pub const DATABASE_READ_TOOLS: &[(&str, &str)] = &[
    ("databases.projects.list", "list_database_projects"),
    ("databases.list", "list_managed_databases"),
    ("databases.get", "get_managed_database"),
    ("databases.permissions", "get_database_permissions"),
    ("databases.plans.list", "list_database_plans"),
    (
        "databases.backupPolicies.get",
        "get_database_backup_history",
    ),
    ("databases.operations.get", "get_database_operation"),
];
fn paginated(action: &str) -> bool {
    matches!(
        action,
        "databases.projects.list"
            | "databases.list"
            | "databases.plans.list"
            | "databases.backupPolicies.get"
    )
}
/// Validated read input, never an authority, provider material or arbitrary native action.
pub struct DatabaseReadCommand {
    pub(crate) command: ManagedCommand,
    pub(crate) limit: u64,
    pub(crate) cursor: String,
}
impl DatabaseReadCommand {
    pub fn parse(mut value: Value) -> Result<Self> {
        if serde_json::to_vec(&value).map_err(|_| invalid())?.len() > 65536 {
            return Err(invalid());
        }
        let action = value["action"].as_str().ok_or_else(invalid)?;
        if !DATABASE_READ_TOOLS.iter().any(|(a, _)| *a == action) {
            return Err(invalid());
        }
        let mut limit = 50;
        let mut cursor = "0".to_owned();
        if paginated(action) {
            let map = value.as_object_mut().ok_or_else(invalid)?;
            if let Some(v) = map.remove("limit") {
                limit = v
                    .as_u64()
                    .filter(|n| (1..=100).contains(n))
                    .ok_or_else(invalid)?;
            }
            if let Some(v) = map.remove("cursor") {
                let s = v.as_str().ok_or_else(invalid)?;
                if s.starts_with('0')
                    || !s.bytes().all(|b| b.is_ascii_digit())
                    || s.parse::<i64>().ok().filter(|id| *id > 0).is_none()
                {
                    return Err(invalid());
                }
                cursor = s.to_owned();
            }
        }
        Ok(Self {
            command: ManagedCommand::parse(value)?,
            limit,
            cursor,
        })
    }
    pub fn action(&self) -> &str {
        self.command.action()
    }
}
/// Tool input schemas are derived from the same strict ManagedCommand alternatives.
pub fn database_read_tools() -> Vec<Value> {
    let document: Value =
        serde_json::from_str(crate::MANAGED_COMMAND_SCHEMA).expect("managed schema");
    DATABASE_READ_TOOLS.iter().map(|(action, name)| {
        let mut schema = document["oneOf"].as_array().expect("command union").iter().find(|s| s["properties"]["action"]["const"] == *action).expect("read action").clone();
        schema["properties"].as_object_mut().expect("command properties").remove("action");
        schema["required"].as_array_mut().expect("required fields").retain(|v| v != "action");
        if paginated(action) {
            schema["properties"]["limit"] = json!({"type":"integer","minimum":1,"maximum":100,"default":50});
            schema["properties"]["cursor"] = json!({"type":"string","pattern":"^[1-9][0-9]{0,18}$","description":"Use the previous response nextCursor; ascending immutable numeric IDs."});
        }
        json!({"name":name,"description":"Read current authorized managed PostgreSQL metadata. Lists/history use items and nextCursor; null nextCursor ends the list. No credentials or recovery activation.","inputSchema":schema,"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}})
    }).collect()
}
