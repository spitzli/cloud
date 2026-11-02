//! Physical PostgreSQL installation storage; never a customer logical database pool.
use super::*;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseStoragePacket {
    pub version: u8,
    #[serde(rename = "installationID")]
    pub installation_id: RecordId,
    #[serde(rename = "clusterID")]
    pub cluster_id: RecordId,
    pub revision: u64,
    pub node: String,
    #[serde(rename = "poolID")]
    pub pool_id: String,
    pub volume_bytes: u64,
    pub lease_until: DateTime<Utc>,
}
impl DatabaseStoragePacket {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || !(1..=2147483646).contains(&self.revision)
            || !(MIN..=MAX).contains(&self.volume_bytes)
            || !valid_uuid(&self.pool_id)
            || self.node.is_empty()
            || self.node.len() > 253
            || !self.node.as_bytes()[0].is_ascii_alphanumeric()
            || !self
                .node
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        {
            return Err(ExecutionError::Invalid("database storage packet"));
        }
        let remaining = self.lease_until - Utc::now();
        if remaining <= chrono::Duration::zero() || remaining > chrono::Duration::seconds(300) {
            return Err(ExecutionError::Expired);
        }
        Ok(())
    }
    fn packet(&self, config: &StorageConfig) -> Result<StoragePacket> {
        self.validate()?;
        if self.cluster_id != config.cluster_id
            || self.node != config.node
            || self.pool_id != config.pool_id
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(StoragePacket {
            app_id: self.installation_id.clone(),
            project_id: self.installation_id.clone(),
            cluster_id: self.cluster_id.clone(),
            revision: self.revision,
            lease_until: self.lease_until,
            spec: StorageSpec {
                volume_bytes: self.volume_bytes,
            },
        })
    }
    pub fn persistent_volume(&self, path: &Path) -> Result<serde_json::Value> {
        self.validate()?;
        if !path.is_absolute()
            || path.components().any(|p| {
                matches!(
                    p,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err(ExecutionError::Invalid("database data path"));
        }
        let name = format!("wd-db-{}", self.installation_id);
        Ok(
            serde_json::json!({"apiVersion":"v1","kind":"PersistentVolume","metadata":{"name":name,"labels":{"app.kubernetes.io/managed-by":"webdock","webdock.dev/database-installation":self.installation_id}},"spec":{"capacity":{"storage":self.volume_bytes.to_string()},"accessModes":["ReadWriteOnce"],"volumeMode":"Filesystem","persistentVolumeReclaimPolicy":"Retain","storageClassName":"","claimRef":{"namespace":name,"name":"postgres-1"},"local":{"path":path},"nodeAffinity":{"required":{"nodeSelectorTerms":[{"matchExpressions":[{"key":"kubernetes.io/hostname","operator":"In","values":[self.node]}]}]}}}}),
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseAllocation {
    #[serde(rename = "installationID")]
    pub installation_id: RecordId,
    pub volume_bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseStorageCapability {
    pub node: String,
    #[serde(rename = "poolID")]
    pub pool_id: String,
    pub capacity_bytes: u64,
    pub ready: bool,
    pub available_bytes: u64,
    pub allocated_bytes: u64,
    pub allocations: Vec<DatabaseAllocation>,
}
impl DatabaseStorageCapability {
    pub fn validate(&self, nodes: &[String]) -> Result<()> {
        if !nodes.contains(&self.node)
            || !valid_uuid(&self.pool_id)
            || [
                self.capacity_bytes,
                self.available_bytes,
                self.allocated_bytes,
            ]
            .iter()
            .any(|bytes| *bytes > 9_007_199_254_740_991)
            || self.allocations.len() > 100
            || self
                .allocations
                .iter()
                .map(|a| a.installation_id.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.allocations.len()
            || self
                .allocations
                .iter()
                .any(|a| !(MIN..=MAX).contains(&a.volume_bytes))
        {
            return Err(ExecutionError::Invalid("database storage capability"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseStorageResult {
    pub version: u8,
    #[serde(rename = "installationID")]
    pub installation_id: RecordId,
    pub node: String,
    #[serde(rename = "poolID")]
    pub pool_id: String,
    pub volume_bytes: u64,
    pub persistent_volume: serde_json::Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseVolumeState {
    pub state: VolumeState,
    pub volume_bytes: u64,
}
fn capability(pool: &StoragePool, nodes: &[String]) -> Result<DatabaseStorageCapability> {
    if nodes.len() > 200
        || nodes.iter().any(|n| n.len() > 253)
        || !nodes.contains(&pool.config.node)
    {
        return Err(ExecutionError::Ownership);
    }
    let state = pool.state()?;
    let guard = StorageGuard::maintenance();
    let mut ready = true;
    let mut allocated_bytes = 0u64;
    let mut allocations = Vec::new();
    for record in state.volumes.values() {
        if record.state != VolumeState::Purged {
            allocated_bytes = allocated_bytes
                .checked_add(record.bytes)
                .ok_or(ExecutionError::Runtime)?;
            allocations.push(DatabaseAllocation {
                installation_id: record.app_id.clone(),
                volume_bytes: record.bytes,
            });
            if record.state != VolumeState::Ready {
                ready = false;
                continue;
            }
            let directory = pool.config.root.join(record.app_id.as_str());
            pool.image(&directory, record, &guard)?;
            if !pool.mounted(&directory, &guard)? {
                ready = false;
                continue;
            }
            let data = directory.join("mount/data");
            let info = io(std::fs::symlink_metadata(&data))?;
            if !info.is_dir() || info.uid() != 26 || info.gid() != 26 {
                ready = false;
            }
        }
    }
    let free = rustix::fs::statvfs(&pool.config.root).map_err(|_| ExecutionError::Runtime)?;
    let available_bytes = free
        .f_bavail
        .checked_mul(free.f_frsize)
        .ok_or(ExecutionError::Runtime)?
        .saturating_sub(pool.config.reserve_bytes);
    Ok(DatabaseStorageCapability {
        node: pool.config.node.clone(),
        pool_id: pool.config.pool_id.clone(),
        capacity_bytes: pool.config.capacity_bytes,
        ready,
        available_bytes,
        allocated_bytes,
        allocations,
    })
}
pub(super) fn dispatch(
    pool: &mut StoragePool,
    request: RpcRequest,
    cancelled: Arc<AtomicBool>,
) -> Result<serde_json::Value> {
    if request.action == "database.capability" {
        let nodes: Vec<String> = serde_json::from_value(request.value)
            .map_err(|_| ExecutionError::Invalid("database nodes"))?;
        return serde_json::to_value(capability(pool, &nodes)?)
            .map_err(|_| ExecutionError::Runtime);
    }
    if !matches!(
        request.action.as_str(),
        "database.ensure" | "database.state" | "database.retained" | "database.purge"
    ) {
        return Err(ExecutionError::Unsupported("database storage action"));
    }
    let value: DatabaseStoragePacket = serde_json::from_value(request.value)
        .map_err(|_| ExecutionError::Invalid("database storage packet"))?;
    let packet = value.packet(&pool.config)?;
    let guard = StorageGuard::new(packet.lease_until, cancelled);
    guard.check()?;
    match request.action.as_str() {
        "database.ensure" => {
            let (_, path) = pool.ensure(&packet, &guard)?;
            guard.check()?;
            serde_json::to_value(DatabaseStorageResult {
                version: 1,
                installation_id: value.installation_id.clone(),
                node: value.node.clone(),
                pool_id: value.pool_id.clone(),
                volume_bytes: value.volume_bytes,
                persistent_volume: value.persistent_volume(&path)?,
            })
            .map_err(|_| ExecutionError::Runtime)
        }
        "database.state" => {
            let record = pool
                .record(&packet.app_id)?
                .ok_or(ExecutionError::Ownership)?;
            record.authorize(&packet)?;
            serde_json::to_value(DatabaseVolumeState {
                state: record.state,
                volume_bytes: record.bytes,
            })
            .map_err(|_| ExecutionError::Runtime)
        }
        "database.retained" => {
            pool.retained(&packet)?;
            Ok(serde_json::Value::Null)
        }
        "database.purge" => {
            pool.purge(&packet, &guard)?;
            Ok(serde_json::Value::Null)
        }
        _ => unreachable!(),
    }
}
#[derive(Clone)]
pub struct DatabaseStorageClient(StorageClient);
impl DatabaseStorageClient {
    pub fn new(socket: PathBuf) -> Self {
        Self(StorageClient::new(socket))
    }
    pub async fn capability(&self, nodes: &[String]) -> Result<DatabaseStorageCapability> {
        let capability: DatabaseStorageCapability = self
            .0
            .call(
                "database.capability",
                serde_json::to_value(nodes)
                    .map_err(|_| ExecutionError::Invalid("database nodes"))?,
            )
            .await?;
        capability.validate(nodes)?;
        Ok(capability)
    }
    pub async fn ensure(&self, packet: &DatabaseStoragePacket) -> Result<DatabaseStorageResult> {
        packet.validate()?;
        let result: DatabaseStorageResult = self
            .0
            .call(
                "database.ensure",
                serde_json::to_value(packet)
                    .map_err(|_| ExecutionError::Invalid("database packet"))?,
            )
            .await?;
        if result.version != 1
            || result.installation_id != packet.installation_id
            || result.node != packet.node
            || result.pool_id != packet.pool_id
            || result.volume_bytes != packet.volume_bytes
        {
            return Err(ExecutionError::Ownership);
        }
        let path = result.persistent_volume["spec"]["local"]["path"]
            .as_str()
            .ok_or(ExecutionError::Ownership)?;
        if result.persistent_volume != packet.persistent_volume(Path::new(path))? {
            return Err(ExecutionError::Ownership);
        }
        Ok(result)
    }
    pub async fn state(&self, packet: &DatabaseStoragePacket) -> Result<DatabaseVolumeState> {
        packet.validate()?;
        self.0
            .call(
                "database.state",
                serde_json::to_value(packet)
                    .map_err(|_| ExecutionError::Invalid("database packet"))?,
            )
            .await
    }
    pub async fn retained(&self, packet: &DatabaseStoragePacket) -> Result<()> {
        packet.validate()?;
        self.0
            .call(
                "database.retained",
                serde_json::to_value(packet)
                    .map_err(|_| ExecutionError::Invalid("database packet"))?,
            )
            .await
    }
    pub async fn purge(&self, packet: &DatabaseStoragePacket) -> Result<()> {
        packet.validate()?;
        self.0
            .call(
                "database.purge",
                serde_json::to_value(packet)
                    .map_err(|_| ExecutionError::Invalid("database packet"))?,
            )
            .await
    }
}
