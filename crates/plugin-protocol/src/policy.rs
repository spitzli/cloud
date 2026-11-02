use crate::{Manifest, PluginError, Result};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Availability {
    Enabled,
    Draining,
    Disabled,
}
impl Availability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Draining => "draining",
            Self::Disabled => "disabled",
        }
    }
}
/// A fresh authority snapshot supplied by the host, never by plugin input.
#[derive(Debug, Clone)]
pub struct ActionPolicy<'a> {
    pub state: Availability,
    pub ready: bool,
    pub generation: u64,
    pub runtime_generation: u64,
    pub configuration_revision: u64,
    pub runtime_configuration_revision: u64,
    pub revoked: bool,
    pub project_active: bool,
    pub project_allowed: bool,
    pub project_can_write: bool,
    pub preview: bool,
    pub approved: &'a [String],
    pub granted: &'a [String],
}
impl ActionPolicy<'_> {
    pub fn authorize<'a>(
        &self,
        manifest: &'a Manifest,
        name: &str,
    ) -> Result<&'a serde_json::Value> {
        let action = manifest.action(name).ok_or_else(PluginError::denied)?;
        if self.state != Availability::Enabled
            || !self.ready
            || self.generation == 0
            || self.generation != self.runtime_generation
            || self.configuration_revision != self.runtime_configuration_revision
            || self.revoked
            || !self.project_active
            || !self.project_allowed
            || self.preview
            || (action["intent"] == "write" && !self.project_can_write)
            || action["capabilities"].as_array().unwrap().iter().any(|c| {
                let c = c.as_str().unwrap();
                !self.approved.iter().any(|x| x == c) || !self.granted.iter().any(|x| x == c)
            })
        {
            return Err(PluginError::denied());
        }
        Ok(action)
    }
}
