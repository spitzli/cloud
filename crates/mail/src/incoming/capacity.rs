use super::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Demand {
    apps: u64,
    cpu_millicores: u64,
    memory_bytes: u64,
    volume_bytes: u64,
    ephemeral_bytes: u64,
    replicas_per_app: u64,
    concurrent_deployments: u64,
}
const DEMAND: Demand = Demand {
    apps: 1,
    cpu_millicores: 500,
    memory_bytes: 1073741824,
    volume_bytes: 10737418240,
    ephemeral_bytes: 268435456,
    replicas_per_app: 1,
    concurrent_deployments: 1,
};
pub(super) async fn complete(
    tx: &DatabaseTransaction,
    customer: &str,
    enabled: bool,
) -> Result<()> {
    let demand = if enabled {
        DEMAND
    } else {
        Demand {
            apps: 0,
            cpu_millicores: 0,
            memory_bytes: 0,
            volume_bytes: DEMAND.volume_bytes,
            ephemeral_bytes: 0,
            replicas_per_app: 0,
            concurrent_deployments: 0,
        }
    };
    crate::outbox::run(tx,"UPDATE webdock_mail.cluster_reservation SET demand=$2,status='active' WHERE customer_id=$1",vec![customer.into(),serde_json::to_value(demand).map_err(|_|Error::Unavailable)?.into()]).await?;
    Ok(())
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    ownership: String,
    verified: bool,
    revoked: Option<bool>,
    last_seen: Option<DateTime<Utc>>,
    capacity: Option<Demand>,
    observation: Option<Observation>,
}
#[derive(Deserialize)]
struct Observation {
    version: String,
    capabilities: Capabilities,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Capabilities {
    native_mail: Option<NativeMail>,
    storage_version: Option<u64>,
}
#[derive(Deserialize)]
struct NativeMail {
    version: u64,
}
impl Demand {
    fn dimensions(&self) -> [u64; 7] {
        [
            self.apps,
            self.cpu_millicores,
            self.memory_bytes,
            self.volume_bytes,
            self.ephemeral_bytes,
            self.replicas_per_app,
            self.concurrent_deployments,
        ]
    }
}
pub(super) async fn reserve(
    tx: &DatabaseTransaction,
    customer: &str,
    cluster: &str,
    local_docker: bool,
) -> Result<()> {
    one(
        tx,
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        vec![format!("hosting-cluster:{cluster}").into()],
    )
    .await?;
    let row = one(
        tx,
        "SELECT webdock_rls.cluster_snapshot($1) AS snapshot",
        vec![cluster.into()],
    )
    .await?
    .ok_or(Error::Unavailable)?;
    let snapshot: Snapshot = decode(&row, "snapshot")?;
    let observation = snapshot.observation.ok_or(Error::Conflict)?;
    let kind = one(
        tx,
        "SELECT runtime_kind FROM webdock_auth.hosting_cluster WHERE id=$1",
        vec![cluster.into()],
    )
    .await?
    .ok_or(Error::Conflict)?;
    let docker = local_docker && field::<String>(&kind, "runtime_kind")? == "docker";
    let fresh = snapshot.last_seen.is_some_and(|seen| {
        let age = Utc::now().signed_duration_since(seen).num_seconds();
        (0..90).contains(&age)
    });
    let k3s = observation.version.starts_with('v')
        && observation
            .version
            .split_once("+k3s")
            .is_some_and(|(_, suffix)| {
                !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
            });
    if snapshot.ownership != "platform"
        || !snapshot.verified
        || snapshot.revoked != Some(false)
        || !fresh
        || (!k3s && !docker)
        || observation
            .capabilities
            .native_mail
            .is_none_or(|mail| mail.version != 1)
        || (!docker && observation.capabilities.storage_version != Some(1))
    {
        return Err(Error::Conflict);
    }
    let limit = snapshot.capacity.ok_or(Error::Conflict)?;
    let used = one(
        tx,
        "SELECT webdock_rls.capacity_demand($1,'all',$2) AS demand",
        vec![cluster.into(), customer.into()],
    )
    .await?
    .ok_or(Error::Unavailable)?;
    let used: Demand = decode(&used, "demand")?;
    for (index, ((used, wanted), limit)) in used
        .dimensions()
        .into_iter()
        .zip(DEMAND.dimensions())
        .zip(limit.dimensions())
        .enumerate()
    {
        let wanted = if index == 5 {
            used.max(wanted)
        } else {
            used.checked_add(wanted).ok_or(Error::Conflict)?
        };
        if wanted > limit || limit > 9_007_199_254_740_991 {
            return Err(Error::Conflict);
        }
    }
    let row=one(tx,"INSERT INTO webdock_mail.cluster_reservation(customer_id,cluster_id,demand,status) VALUES($1,$2,$3,'reserved') ON CONFLICT(customer_id) DO UPDATE SET demand=excluded.demand,status='reserved' WHERE cluster_reservation.cluster_id=excluded.cluster_id RETURNING customer_id",vec![customer.into(),cluster.into(),serde_json::to_value(DEMAND).map_err(|_| Error::Unavailable)?.into()]).await?;
    if row.is_none() {
        return Err(Error::Conflict);
    }
    Ok(())
}
