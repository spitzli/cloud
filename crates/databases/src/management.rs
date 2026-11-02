use crate::{Result, invalid};
use serde_json::Value;
use std::sync::OnceLock;
/// Retained managed database command schema. Cross-field plan refinements are checked below.
pub struct ManagedCommand(Value);
impl ManagedCommand {
    pub fn parse(mut value: Value) -> Result<Self> {
        fn normalize(value: &mut Value) {
            match value {
                Value::Object(map) => {
                    for child in map.values_mut() {
                        normalize(child)
                    }
                }
                Value::Array(items) => {
                    for child in items {
                        normalize(child)
                    }
                }
                Value::Number(number) => {
                    if let Some(n) = number.as_f64()
                        && n.is_finite()
                        && n.fract() == 0.0
                        && (0.0..=9_007_199_254_740_991.0).contains(&n)
                    {
                        *value = json_number(n as u64);
                    }
                }
                _ => {}
            }
        }
        fn json_number(value: u64) -> Value {
            Value::Number(value.into())
        }
        normalize(&mut value);

        if serde_json::to_vec(&value).map_err(|_| invalid())?.len() > 65536 {
            return Err(invalid());
        }
        for key in ["name", "confirmName"] {
            if let Some(s) = value.get(key).and_then(Value::as_str).map(str::to_owned) {
                value[key] = Value::String(s.trim().into());
            }
        }
        if let Some(plan) = value.get_mut("plan") {
            for key in ["name", "terms"] {
                if let Some(s) = plan.get(key).and_then(Value::as_str).map(str::to_owned) {
                    plan[key] = Value::String(s.trim().into());
                }
            }
        }
        static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
        let validator = VALIDATOR.get_or_init(|| {
            jsonschema::options()
                .should_validate_formats(true)
                .build(
                    &serde_json::from_str(crate::MANAGED_COMMAND_SCHEMA)
                        .expect("retained command schema"),
                )
                .expect("valid managed schema")
        });
        if !validator.is_valid(&value) {
            return Err(invalid());
        }
        fn identifiers(value: &Value) -> Result<()> {
            if let Some(object) = value.as_object() {
                for (key, child) in object {
                    if key.ends_with("ID")
                        && key != "poolID"
                        && let Some(id) = child.as_str()
                        && id.parse::<i64>().is_err()
                    {
                        return Err(invalid());
                    }
                    identifiers(child)?;
                }
            } else if let Some(array) = value.as_array() {
                for child in array {
                    identifiers(child)?;
                }
            }
            Ok(())
        }
        identifiers(&value)?;
        if let Some(plan) = value.get("plan") {
            validate_plan(plan)?;
        }
        Ok(Self(value))
    }
    pub fn value(&self) -> &Value {
        &self.0
    }
    pub fn into_value(self) -> Value {
        self.0
    }
    pub fn action(&self) -> &str {
        self.0["action"].as_str().expect("validated command")
    }
    pub fn is_read(&self) -> bool {
        [
            "databases.backupPolicies.get",
            "databases.projects.list",
            "databases.migrations.list",
            "databases.migrationPurgePreview",
            "databases.permissions",
            "databases.operations.get",
            "databases.list",
            "databases.get",
            "databases.plans.list",
            "databases.installations.list",
            "databases.deletionPreview",
            "databases.purgePreview",
        ]
        .contains(&self.action())
    }
}
pub(crate) fn validate_plan(plan: &Value) -> Result<()> {
    static PLAN: OnceLock<jsonschema::Validator> = OnceLock::new();
    let validator = PLAN.get_or_init(|| {
        let schema: Value =
            serde_json::from_str(crate::MANAGED_COMMAND_SCHEMA).expect("retained schema");
        let plan = schema["oneOf"]
            .as_array()
            .expect("command union")
            .iter()
            .find(|v| v["properties"]["action"]["const"] == "databases.plans.create")
            .expect("plan command")["properties"]["plan"]
            .clone();
        jsonschema::options()
            .should_validate_formats(true)
            .build(&plan)
            .expect("plan schema")
    });
    if !validator.is_valid(plan) {
        return Err(invalid());
    }

    let shared = plan["kind"] == "shared";
    let physical = ["cpuMillicores", "memoryBytes", "volumeBytes"]
        .iter()
        .all(|key| plan[*key].is_null() == shared);
    if !physical || plan["logicalStorageAllowanceBytes"].is_null() == shared {
        return Err(invalid());
    }
    let price = plan["monthlyPriceCents"].as_u64().ok_or_else(invalid)?;
    if plan.get("sponsoredCustomerID").is_some() != (price == 0) {
        return Err(invalid());
    }
    Ok(())
}

/// Installation-owned settings. No provider clients or credentials are constructed from browser input.
#[derive(Clone, Default)]
pub struct ManagementConfig {
    pub cluster_id: Option<String>,
    pub endpoint_domain: Option<String>,
    pub public_ordering: bool,
    pub protected_database_id: Option<String>,
    pub lease_secret: Option<String>,
}
