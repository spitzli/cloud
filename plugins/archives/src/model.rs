use serde::{Deserialize, Serialize};
use webdock_plugin_protocol::storage::StorageAuthorization;
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Diagnostics,
    Postgres,
    Project,
    Report,
    #[serde(rename = "buildlog")]
    BuildLog,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Pending,
    Producing,
    Ready,
    Deleting,
    Deleted,
    Failed,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    ReadonlySourceUnavailable,
    SourceAccessChanged,
    OutputLimit,
    StorageUnavailable,
    ExportFailed,
}
impl Failure {
    pub fn from_error(error: &str) -> Self {
        match error {
            "Readonly source role is not reconciled or TLS verification failed"
            | "Readonly database source is not reconciled or authorized"
            | "Source role has unsafe privileges" => Self::ReadonlySourceUnavailable,
            "Export source authority changed"
            | "Export source binding changed"
            | "Archive source binding changed" => Self::SourceAccessChanged,
            "Managed export limit"
            | "Export source limit"
            | "Compressed source limit"
            | "Build log output limit"
            | "Database export exceeds limit" => Self::OutputLimit,
            "Archive upload unavailable" | "Archive signing unavailable" => {
                Self::StorageUnavailable
            }
            _ => Self::ExportFailed,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Archive {
    pub id: uuid::Uuid,
    pub customer: String,
    pub project: String,
    pub kind: Kind,
    pub state: State,
    pub epoch: String,
    pub expires_at: i64,
    pub source_binding: Option<String>,
    pub core_source: Option<webdock_export_protocol::SourceRef>,
    pub job: Option<String>,
    pub failure: Option<Failure>,
    pub receipt: Option<webdock_object_storage::ObjectReceipt>,
    pub diagnostics: Option<crate::diagnostics::Diagnostics>,
}
impl Archive {
    pub fn scope(&self) -> String {
        format!("{}/{}/{}", self.customer, self.project, self.id)
    }
    pub fn owned(&self, customer: &str, project: &str) -> crate::Result<()> {
        if self.customer != customer || self.project != project {
            return Err("Archive unavailable");
        }
        Ok(())
    }
    /// Historical unqualified exports stay inspectable/deletable, never byte-readable.
    pub fn source(&self) -> crate::Result<Option<&webdock_export_protocol::SourceRef>> {
        use webdock_export_protocol::SourceKind;
        let expected = match self.kind {
            Kind::Diagnostics if self.core_source.is_none() => return Ok(None),
            Kind::Diagnostics => return Err("Invalid source kind"),
            Kind::Postgres => SourceKind::Postgres,
            Kind::Project => SourceKind::Project,
            Kind::Report => SourceKind::Report,
            Kind::BuildLog => SourceKind::BuildLog,
        };
        let source = self
            .core_source
            .as_ref()
            .ok_or("Core export source required")?;
        source.validate()?;
        if source.kind != expected
            || self.source_binding.as_deref() != Some(source.binding.as_str())
        {
            return Err("Source binding mismatch");
        }
        Ok(Some(source))
    }
    pub fn ready(&self, now: i64) -> crate::Result<&webdock_object_storage::ObjectReceipt> {
        if self.state != State::Ready || self.expires_at <= now {
            return Err("Archive unavailable");
        }
        if let Some(source) = self.source()? {
            source.finished()?;
        }
        let receipt = self.receipt.as_ref().ok_or("Archive unavailable")?;
        if self.kind == Kind::BuildLog
            && receipt.size_bytes > webdock_export_protocol::MAX_BUILD_LOG_GZIP_BYTES
        {
            return Err("Build log output limit");
        }
        Ok(receipt)
    }
    pub fn authorize(&self, a: &StorageAuthorization, now: i64) -> crate::Result<()> {
        let r = self.ready(now)?;
        self.owned(&a.customer_id, &a.project_id)?;
        if a.plugin_id != "archives"
            || a.action != "archives.download"
            || a.direction != webdock_plugin_protocol::storage::StorageDirection::Download
            || a.resource_id != self.id.to_string()
            || a.resource_epoch != self.epoch
            || a.exact_bytes != r.size_bytes
            || a.expected_sha256.as_deref() != Some(r.sha256.as_str())
            || a.expires_at > self.expires_at
            || a.expires_at <= now
        {
            return Err("Archive unavailable");
        }
        r.validate(&self.scope(), crate::MAX_BYTES)
            .map_err(|_| "Archive receipt damaged")?;
        Ok(())
    }
}
