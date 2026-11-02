//! Browser-safe runtime observations; execution envelopes and receipts stay private.
use crate::{Result, error};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use webdock_core::id::RecordId;
use webdock_supervisor::{ObservedState, ResourceMetrics};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    NeedsReconciliation,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogOutput {
    pub logs: String,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ReadOutput {
    Logs(LogOutput),
    Inspect(ObservedState),
    Metrics(ResourceMetrics),
}

#[derive(Debug, Serialize)]
pub struct ReadView {
    pub id: RecordId,
    pub status: ReadStatus,
    pub result: Option<ReadOutput>,
    pub error: Option<&'static str>,
}

impl ReadView {
    pub(crate) fn from_stored(
        id: &str,
        action: &str,
        status: Value,
        stored: &Value,
    ) -> Result<Self> {
        let invalid = || error(503, "Stored runtime observation is unavailable.");
        let status: ReadStatus = serde_json::from_value(status).map_err(|_| invalid())?;
        let result = if status == ReadStatus::Succeeded {
            let value = stored["readResult"].clone();
            Some(match action {
                "apps.logs" => {
                    ReadOutput::Logs(serde_json::from_value(value).map_err(|_| invalid())?)
                }
                "apps.inspect" => {
                    ReadOutput::Inspect(serde_json::from_value(value).map_err(|_| invalid())?)
                }
                "apps.metrics" => {
                    ReadOutput::Metrics(serde_json::from_value(value).map_err(|_| invalid())?)
                }
                _ => return Err(invalid()),
            })
        } else {
            None
        };
        Ok(Self {
            id: RecordId::parse(id).map_err(|_| invalid())?,
            status,
            result,
            error: matches!(status, ReadStatus::Failed | ReadStatus::NeedsReconciliation)
                .then_some("Application read failed."),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn completed_observation_must_match_its_recorded_action() {
        let logs = json!({"readResult":{"logs":"safe","truncated":false}});
        assert!(ReadView::from_stored("123", "apps.logs", json!("succeeded"), &logs).is_ok());
        for (action, status, stored) in [
            ("apps.inspect", "succeeded", logs.clone()),
            ("apps.metrics", "succeeded", logs),
            ("apps.logs", "succeeded", json!({})),
            ("apps.logs", "unrecognized", json!({})),
            (
                "apps.logs",
                "succeeded",
                json!({"readResult":{
                    "logs":"safe","truncated":false,"authorization":"private"
                }}),
            ),
        ] {
            assert!(ReadView::from_stored("123", action, json!(status), &stored).is_err());
        }
    }

    #[test]
    fn pending_and_failed_reads_never_forward_internal_receipts_or_errors() {
        let stored = json!({
            "error":"internal credential detail",
            "nativeReadReceipt":{"resultMac":"private"},
            "readResult":{"logs":"partial private output"}
        });
        for status in ["queued", "running", "failed", "needs-reconciliation"] {
            let view = ReadView::from_stored("123", "apps.logs", json!(status), &stored).unwrap();
            let serialized = serde_json::to_value(view).unwrap();
            assert!(serialized["result"].is_null());
            assert!(!serialized.to_string().contains("private"));
            assert!(!serialized.to_string().contains("credential"));
        }
    }
}
