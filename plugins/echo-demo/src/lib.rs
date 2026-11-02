//! Reference extension: no capabilities, host services or external providers.
//! Echoes messages for operators and offers demo notes as a self-service resource. Notes keep no
//! state here: the Core resource index holds their names, so they are a pure fixture.
use serde::Deserialize;
use serde_json::{Value, json};
use webdock_plugin_protocol::{ErrorCode, InvocationClaims, Manifest, PluginError};
use webdock_plugin_runtime::ActionHandler;

type Result<T> = std::result::Result<T, PluginError>;
pub const PLUGIN_ID: &str = "echo-demo";
const ACTIONS: &[(&str, &str)] = &[
    ("demo.echo", "read"),
    ("demo.operator", "read"),
    ("notes.create", "write"),
    ("notes.view", "read"),
    ("notes.secret", "read"),
    ("notes.delete", "write"),
];
fn invalid() -> PluginError {
    PluginError::new(ErrorCode::InvalidInput, "Invalid echo request")
}
fn text(value: &str, min: usize, max: usize) -> Result<&str> {
    let count = value.chars().count();
    if count < min || count > max || value.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(value)
}
fn parse<T: for<'a> Deserialize<'a>>(input: Value) -> Result<T> {
    serde_json::from_value(input).map_err(|_| invalid())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    #[serde(default)]
    message: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewNote {
    name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Note {
    resource_id: String,
}
impl Note {
    fn id(&self) -> Result<&str> {
        uuid::Uuid::parse_str(&self.resource_id).map_err(|_| invalid())?;
        Ok(&self.resource_id)
    }
}
pub struct Echo;
#[tonic::async_trait]
impl ActionHandler for Echo {
    async fn invoke(
        &self,
        claims: &InvocationClaims,
        _token: &str,
        input: Value,
        configuration: Value,
    ) -> Result<Value> {
        if configuration != json!({}) {
            return Err(invalid());
        }
        match claims.action.as_str() {
            "demo.echo" => {
                let input: Message = parse(input)?;
                Ok(json!({"message":text(&input.message, 0, 200)?,"projectID":claims.project_id}))
            }
            "demo.operator" => Ok(json!({"version":2,"title":"Echo demo","blocks":[
                {"kind":"status","label":"Runtime","state":"healthy","text":"Answering in this project"}
            ]})),
            "notes.create" => {
                let input: NewNote = parse(input)?;
                let name = text(&input.name, 1, 80)?;
                Ok(json!({"resource":{"id":uuid::Uuid::new_v4().to_string(),"name":name}}))
            }
            "notes.view" => {
                let note: Note = parse(input)?;
                note.id()?;
                Ok(json!({"version":2,"title":"Note","blocks":[
                    {"kind":"status","label":"State","state":"healthy","text":"Ready"},
                    {"kind":"reveal","label":"Access code","action":"notes.secret"},
                    {"kind":"actions","actions":[{"id":"notes.delete","label":"Delete note","destructive":true}]}
                ]}))
            }
            "notes.secret" => {
                let note: Note = parse(input)?;
                Ok(json!({"value":format!("echo:{}", note.id()?)}))
            }
            "notes.delete" => {
                let note: Note = parse(input)?;
                note.id()?;
                Ok(json!({}))
            }
            _ => Err(PluginError::new(
                ErrorCode::NotSupported,
                "Echo action unavailable",
            )),
        }
    }
}
/// The runtime refuses to start with any manifest other than the reviewed demo contract.
pub fn validate_manifest(manifest: &Manifest) -> Result<()> {
    let value = manifest.as_json();
    if manifest.id() != PLUGIN_ID
        || value["capabilities"] != json!([])
        || value["httpBindings"] != json!([])
        || value["actions"]
            .as_array()
            .is_none_or(|a| a.len() != ACTIONS.len())
        || !ACTIONS.iter().all(|(name, intent)| {
            manifest.action(name).is_some_and(|action| {
                action["intent"] == *intent && action["capabilities"] == json!([])
            })
        })
    {
        return Err(invalid());
    }
    Ok(())
}
