use crate::ContractError;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeatureState {
    Enabled,
    Draining,
    Disabled,
}

#[derive(Debug, Clone, Copy)]
pub enum OperationKind {
    Read,
    Provision,
    ContinueAccepted,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(transparent)]
pub struct FeatureSet(BTreeMap<String, FeatureState>);

pub const FEATURES: &[&str] = &[
    "hosting",
    "databases",
    "mail",
    "storage",
    "cms",
    "observability",
];

impl FeatureSet {
    pub fn from_json(value: &str) -> Result<Self, ContractError> {
        let states: BTreeMap<String, FeatureState> = serde_json::from_str(value)
            .map_err(|_| ContractError("Invalid feature configuration".into()))?;
        if states.keys().any(|name| !FEATURES.contains(&name.as_str())) {
            return Err(ContractError("Unknown built-in feature".into()));
        }
        Ok(Self(states))
    }
    pub fn state(&self, feature: &str) -> FeatureState {
        self.0
            .get(feature)
            .copied()
            .unwrap_or(FeatureState::Disabled)
    }
    pub fn visible(&self) -> Vec<String> {
        self.0
            .iter()
            .filter(|(_, state)| **state != FeatureState::Disabled)
            .map(|(name, _)| name.clone())
            .collect()
    }
    /// This is admission only; accepted work still requires a live operation lease.
    pub fn require(&self, feature: &str, operation: OperationKind) -> Result<(), ContractError> {
        match (self.state(feature), operation) {
            (FeatureState::Enabled, _)
            | (FeatureState::Draining, OperationKind::Read | OperationKind::ContinueAccepted) => {
                Ok(())
            }
            (FeatureState::Draining, _) => Err(ContractError("Feature is draining".into())),
            (FeatureState::Disabled, _) => Err(ContractError("Feature is disabled".into())),
        }
    }
}
