//! An allowlist, not a best-effort redactor. No free text enters this archive.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostics {
    version: u8,
    state: Finished,
    builds: Vec<Build>,
    checks: Vec<Check>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Finished {
    Finished,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Build {
    id: String,
    outcome: BuildOutcome,
    duration_ms: u64,
    output_bytes: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum BuildOutcome {
    Succeeded,
    Failed,
    Cancelled,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Check {
    component: Component,
    outcome: CheckOutcome,
    duration_ms: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Component {
    Storage,
    Database,
    Runtime,
    Gateway,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CheckOutcome {
    Healthy,
    Unavailable,
    Denied,
    Integrity,
}
impl Diagnostics {
    pub fn parse(value: Value) -> crate::Result<Self> {
        let d: Self =
            serde_json::from_value(value).map_err(|_| "Invalid structured diagnostics")?;
        if d.version != 1
            || d.builds.len() > 100
            || d.checks.len() > 32
            || d.builds.iter().any(|b| {
                b.id.is_empty()
                    || b.id.len() > 19
                    || b.id.starts_with('0')
                    || !b.id.bytes().all(|b| b.is_ascii_digit())
                    || b.duration_ms > 86_400_000
                    || b.output_bytes > 64 * 1024 * 1024 * 1024
            })
            || d.checks.iter().any(|c| c.duration_ms > 86_400_000)
        {
            return Err("Structured diagnostics limit");
        }
        Ok(d)
    }
    pub fn compress(&self) -> crate::Result<Vec<u8>> {
        // Revalidate private persisted input as well as action input.
        let value = serde_json::to_value(self).map_err(|_| "Invalid diagnostics")?;
        Self::parse(value)?;
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        serde_json::to_writer(&mut gzip, self).map_err(|_| "Diagnostics compression failed")?;
        gzip.flush().map_err(|_| "Diagnostics compression failed")?;
        gzip.finish().map_err(|_| "Diagnostics compression failed")
    }
}
