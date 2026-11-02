use serde::{Deserialize, Serialize};
use uuid::Uuid;
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub id: Uuid,
    pub customer: String,
    pub project: String,
    pub environment: String,
    pub backend: String,
    pub bucket: String,
    pub user: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lifecycle {
    pub expire_current_days: Option<i32>,
    pub expire_noncurrent_days: Option<i32>,
    pub abort_multipart_days: Option<i32>,
}
impl Lifecycle {
    pub fn validate(&self) -> bool {
        [
            self.expire_current_days,
            self.expire_noncurrent_days,
            self.abort_multipart_days,
        ]
        .into_iter()
        .flatten()
        .all(|n| (1..=36500).contains(&n))
    }
    pub fn empty(&self) -> bool {
        *self == Self::default()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub exists: bool,
    pub owned: bool,
    pub versioning: bool,
    pub lifecycle: Option<Lifecycle>,
    pub observed_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UploadCursor {
    pub key: String,
    pub upload: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UploadItem {
    pub key: String,
    pub upload: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UploadBatch {
    pub pending: Vec<UploadItem>,
    pub after: Option<UploadCursor>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Cursor {
    Versions {
        key: Option<String>,
        version: Option<String>,
    },
    Uploads {
        key: Option<String>,
        upload: Option<String>,
    },
    Parts {
        key: String,
        upload: String,
        part: Option<String>,
        batch: UploadBatch,
    },
    Batch(UploadBatch),
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InventoryPage {
    pub current_bytes: u64,
    pub noncurrent_bytes: u64,
    pub multipart_bytes: u64,
    pub current_objects: u64,
    pub noncurrent_versions: u64,
    pub delete_markers: u64,
    pub multipart_uploads: u64,
    pub next: Option<Cursor>,
}
impl InventoryPage {
    pub fn empty(&self) -> bool {
        self.current_objects == 0
            && self.noncurrent_versions == 0
            && self.delete_markers == 0
            && self.multipart_uploads == 0
            && self.current_bytes == 0
            && self.noncurrent_bytes == 0
            && self.multipart_bytes == 0
            && self.next.is_none()
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Inventory {
    pub totals: InventoryPage,
    pub started_at: u64,
    pub observed_at: u64,
    pub complete: bool,
}
// Deliberately no Debug or Serialize: a key may leave only through explicit reveal.
pub struct SecretKey {
    pub access_key: String,
    pub secret_key: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Credential {
    pub access_key: String,
    pub sealed: Vec<u8>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Resource {
    pub binding: Binding,
    pub name: String,
    pub revision: u64,
    pub state: String,
    pub provisioned: bool,
    pub credentials: Vec<Credential>,
    pub observed: Option<Observation>,
    pub inventory: Option<Inventory>,
    pub pending: Option<String>,
    // A lost/cancelled execution is never automatically overtaken, even after its SQL lock dies.
    #[serde(default)]
    pub executing: bool,
    #[serde(default)]
    pub returned_key: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderError {
    Unknown,
    FenceLost,
    ScanInvalidated,
    Collision,
    Ownership,
    Unavailable,
}
pub type ProviderResult<T> = Result<T, ProviderError>;
