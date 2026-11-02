//! Private immutable request/receipt channel; source Pods have no access to either namespace.
use super::{StorageGuard, build::BuildStorageTrust, build_kubernetes::BuildVolumeReconciler};
use k8s_openapi::api::core::v1::ConfigMap;
use kube::{
    Api, Client,
    api::{DeleteParams, ListParams, PostParams, Preconditions},
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use webdock_agent_protocol::build_storage::{
    BuildVolumeFailure, BuildVolumePhase, BuildVolumeReply, SignedBuildStorageReservation,
};
use webdock_supervisor::{ExecutionError, Result};

fn invalid() -> ExecutionError {
    ExecutionError::Invalid("build storage queue")
}
fn queue_namespace(value: &str) -> bool {
    value.starts_with("webdock-build-storage-")
        && value.len() <= 63
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
pub fn parse_request(
    value: &Value,
    namespace: &str,
) -> Result<(SignedBuildStorageReservation, String)> {
    let meta = &value["metadata"];
    if !queue_namespace(namespace)
        || meta["namespace"] != namespace
        || value["immutable"] != true
        || !meta["deletionTimestamp"].is_null()
        || value
            .get("binaryData")
            .is_some_and(|v| !v.is_null() && v.as_object().is_none_or(|o| !o.is_empty()))
        || value["data"]
            .as_object()
            .is_none_or(|o| o.len() != 1 || !o.contains_key("request.json"))
    {
        return Err(invalid());
    }
    let bytes = value["data"]["request.json"]
        .as_str()
        .filter(|s| s.len() <= 8192)
        .ok_or_else(invalid)?;
    let signed: SignedBuildStorageReservation =
        serde_json::from_str(bytes).map_err(|_| invalid())?;
    let r = &signed.reservation;
    r.validate_shape().map_err(|_| invalid())?;
    let uid = meta["uid"]
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        })
        .ok_or_else(invalid)?;
    let labels = json!({"webdock.dev/component":"build-volume-request","webdock.dev/volume-id":r.volume_id,"webdock.dev/generation":r.generation.to_string(),"webdock.dev/epoch":r.epoch.to_string()});
    if meta["name"] != r.queue_name().map_err(|_| invalid())?
        || meta["annotations"]["webdock.dev/request-sha256"]
            != r.fingerprint().map_err(|_| invalid())?
        || meta["labels"] != labels
    {
        return Err(ExecutionError::Ownership);
    }
    Ok((signed, uid.into()))
}
pub struct BuildVolumeQueue {
    client: Client,
    execution_namespace: String,
    requests_namespace: String,
    receipts_namespace: String,
}
impl BuildVolumeQueue {
    pub fn new(
        client: Client,
        execution_namespace: String,
        requests_namespace: String,
        receipts_namespace: String,
    ) -> Result<Self> {
        if !queue_namespace(&requests_namespace)
            || !queue_namespace(&receipts_namespace)
            || requests_namespace == receipts_namespace
            || execution_namespace == requests_namespace
            || execution_namespace == receipts_namespace
        {
            return Err(invalid());
        }
        Ok(Self {
            client,
            execution_namespace,
            requests_namespace,
            receipts_namespace,
        })
    }
    pub async fn poll(
        &self,
        node: &mut BuildVolumeReconciler,
        trust: &BuildStorageTrust,
        stopping: Arc<AtomicBool>,
    ) -> Result<usize> {
        let requests: Api<ConfigMap> =
            Api::namespaced(self.client.clone(), &self.requests_namespace);
        let receipts: Api<ConfigMap> =
            Api::namespaced(self.client.clone(), &self.receipts_namespace);
        let list = tokio::time::timeout(
            Duration::from_secs(10),
            requests.list(
                &ListParams::default()
                    .labels("webdock.dev/component=build-volume-request")
                    .limit(256),
            ),
        )
        .await
        .map_err(|_| ExecutionError::Expired)?
        .map_err(|_| ExecutionError::Runtime)?;
        if list.metadata.continue_.is_some_and(|s| !s.is_empty()) {
            return Err(ExecutionError::Conflict);
        }
        let mut pending = Vec::new();
        for item in list.items {
            let value = serde_json::to_value(item).map_err(|_| invalid())?;
            match parse_request(&value, &self.requests_namespace) {
                Ok((signed, uid)) => pending.push((signed, uid)),
                Err(_) => tracing::warn!("Rejected malformed build volume request metadata"),
            }
        }
        pending.sort_by(|(a, _), (b, _)| {
            (&a.reservation.volume_id, a.reservation.epoch)
                .cmp(&(&b.reservation.volume_id, b.reservation.epoch))
        });
        let mut published = 0;
        for (signed, uid) in pending {
            let r = &signed.reservation;
            if r.target != trust.target {
                continue;
            }
            let name = r.queue_name().map_err(|_| invalid())?;
            if let Some(existing) =
                tokio::time::timeout(Duration::from_secs(10), receipts.get_opt(&name))
                    .await
                    .map_err(|_| ExecutionError::Expired)?
                    .map_err(|_| ExecutionError::Runtime)?
            {
                self.reply(&existing, r, &uid)?;
                continue;
            }
            let guard = StorageGuard::new(r.expires_at, stopping.clone());
            let (result, failure) = match node.reconcile(&signed, trust, &guard).await {
                Ok(result)
                    if matches!(
                        result.phase,
                        BuildVolumePhase::Pending | BuildVolumePhase::Retiring
                    ) =>
                {
                    continue;
                }
                Ok(result) => (Some(result), None),
                Err(ExecutionError::Expired) => (None, Some(BuildVolumeFailure::Expired)),
                Err(ExecutionError::Fenced) => (None, Some(BuildVolumeFailure::Fenced)),
                Err(ExecutionError::Denied | ExecutionError::Invalid(_)) => {
                    (None, Some(BuildVolumeFailure::InvalidAuthority))
                }
                Err(ExecutionError::Ownership) => (None, Some(BuildVolumeFailure::StorageConflict)),
                // Interrupted API or cleanup retains ownership. Retry the same
                // operation, never publish absence or create a new allocation.
                Err(_) => continue,
            };
            let reply = BuildVolumeReply {
                version: 1,
                request_uid: uid.clone(),
                request_fingerprint: r.fingerprint().map_err(|_| invalid())?,
                result,
                failure_code: failure,
            };
            reply
                .validate_for(r, &uid, &self.execution_namespace)
                .map_err(|_| invalid())?;
            let body = serde_json::to_string(&reply).map_err(|_| invalid())?;
            if body.len() > 16384 {
                return Err(invalid());
            }
            // Request deletion is not physical retirement. It does mean no new
            // response should be created for that withdrawn operation.
            let current = tokio::time::timeout(Duration::from_secs(10), requests.get_opt(&name))
                .await
                .map_err(|_| ExecutionError::Expired)?
                .map_err(|_| ExecutionError::Runtime)?;
            if current.as_ref().and_then(|c| c.metadata.uid.as_ref()) != Some(&uid) {
                continue;
            }
            let cm:ConfigMap=serde_json::from_value(json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":name,"namespace":self.receipts_namespace,
                "labels":{"webdock.dev/component":"build-volume-reply","webdock.dev/volume-id":r.volume_id},
                "annotations":{"webdock.dev/request-sha256":reply.request_fingerprint}},"immutable":true,"data":{"reply.json":body}})).map_err(|_|invalid())?;
            match tokio::time::timeout(
                Duration::from_secs(10),
                receipts.create(&PostParams::default(), &cm),
            )
            .await
            .map_err(|_| ExecutionError::Expired)?
            {
                Ok(_) => published += 1,
                Err(kube::Error::Api(e)) if e.code == 409 => {
                    let existing =
                        tokio::time::timeout(Duration::from_secs(10), receipts.get(&name))
                            .await
                            .map_err(|_| ExecutionError::Expired)?
                            .map_err(|_| ExecutionError::Runtime)?;
                    if serde_json::to_value(self.reply(&existing, r, &uid)?)
                        .map_err(|_| invalid())?
                        != serde_json::to_value(reply).map_err(|_| invalid())?
                    {
                        return Err(ExecutionError::Ownership);
                    }
                }
                Err(_) => return Err(ExecutionError::Runtime),
            }
        }
        // A request may disappear after the last check while an API response is
        // committing. Reap only our exact orphan metadata; never infer PV absence.
        let orphan_list = tokio::time::timeout(
            Duration::from_secs(10),
            receipts.list(
                &ListParams::default()
                    .labels("webdock.dev/component=build-volume-reply")
                    .limit(512),
            ),
        )
        .await
        .map_err(|_| ExecutionError::Expired)?
        .map_err(|_| ExecutionError::Runtime)?;
        if orphan_list
            .metadata
            .continue_
            .is_some_and(|s| !s.is_empty())
        {
            return Err(ExecutionError::Conflict);
        }
        for object in orphan_list.items {
            let Some(name) = object.metadata.name.as_deref() else {
                continue;
            };
            let Some(data) = object
                .data
                .as_ref()
                .and_then(|d| d.get("reply.json"))
                .filter(|s| s.len() <= 16384)
            else {
                continue;
            };
            let Ok(reply) = serde_json::from_str::<BuildVolumeReply>(data) else {
                continue;
            };
            let expected = object
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get("webdock.dev/request-sha256"));
            if object.immutable != Some(true)
                || expected != Some(&reply.request_fingerprint)
                || !name.starts_with("wd-vq-")
            {
                continue;
            }
            let current = tokio::time::timeout(Duration::from_secs(10), requests.get_opt(name))
                .await
                .map_err(|_| ExecutionError::Expired)?
                .map_err(|_| ExecutionError::Runtime)?;
            if current
                .as_ref()
                .is_some_and(|c| c.metadata.uid.as_deref() == Some(reply.request_uid.as_str()))
            {
                continue;
            }
            let params = DeleteParams {
                preconditions: Some(Preconditions {
                    uid: object.metadata.uid.clone(),
                    resource_version: object.metadata.resource_version.clone(),
                }),
                ..Default::default()
            };
            if params
                .preconditions
                .as_ref()
                .is_none_or(|p| p.uid.is_none() || p.resource_version.is_none())
            {
                return Err(ExecutionError::Ownership);
            }
            tokio::time::timeout(Duration::from_secs(10), receipts.delete(name, &params))
                .await
                .map_err(|_| ExecutionError::Expired)?
                .map_err(|_| ExecutionError::Runtime)?;
        }
        Ok(published)
    }
    fn reply(
        &self,
        object: &ConfigMap,
        request: &webdock_agent_protocol::build_storage::BuildStorageReservation,
        uid: &str,
    ) -> Result<BuildVolumeReply> {
        if object.immutable != Some(true)
            || object.metadata.namespace.as_deref() != Some(self.receipts_namespace.as_str())
            || object.metadata.deletion_timestamp.is_some()
            || object.binary_data.as_ref().is_some_and(|b| !b.is_empty())
            || object.data.as_ref().is_none_or(|d| d.len() != 1)
        {
            return Err(ExecutionError::Ownership);
        }
        let bytes = object
            .data
            .as_ref()
            .and_then(|d| d.get("reply.json"))
            .filter(|s| s.len() <= 16384)
            .ok_or_else(invalid)?;
        let reply: BuildVolumeReply = serde_json::from_str(bytes).map_err(|_| invalid())?;
        reply
            .validate_for(request, uid, &self.execution_namespace)
            .map_err(|_| ExecutionError::Ownership)?;
        Ok(reply)
    }
}
