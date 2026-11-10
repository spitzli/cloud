//! Signed node storage requests over private, separately authorized Kubernetes namespaces.
use crate::{Result, error};
use ed25519_dalek::SigningKey;
use k8s_openapi::api::core::v1::ConfigMap;
use kube::{
    Api, Client,
    api::{DeleteParams, PostParams, Preconditions},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use spitzli_agent_protocol::build_storage::{
    BuildStorageReservation, BuildStorageTarget, BuildVolumeReply, SignedBuildStorageReservation,
};
use std::time::Duration;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeOperation {
    pub signed: SignedBuildStorageReservation,
    #[serde(default)]
    pub request_uid: Option<String>,
}
#[derive(Clone)]
pub struct KubernetesBuildStorage {
    client: Client,
    execution_namespace: String,
    requests_namespace: String,
    receipts_namespace: String,
    target: BuildStorageTarget,
    signer: SigningKey,
}
fn queue_namespace(value: &str) -> bool {
    value.starts_with("webdock-build-storage-")
        && value.len() <= 63
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
impl KubernetesBuildStorage {
    pub fn new(
        client: Client,
        execution_namespace: String,
        requests_namespace: String,
        receipts_namespace: String,
        target: BuildStorageTarget,
        signer: SigningKey,
    ) -> Result<Self> {
        target
            .validate()
            .map_err(|_| error("invalid_build_storage_target"))?;
        if !queue_namespace(&requests_namespace)
            || !queue_namespace(&receipts_namespace)
            || requests_namespace == receipts_namespace
            || execution_namespace == requests_namespace
            || execution_namespace == receipts_namespace
        {
            return Err(error("invalid_build_storage_namespaces"));
        }
        Ok(Self {
            client,
            execution_namespace,
            requests_namespace,
            receipts_namespace,
            target,
            signer,
        })
    }
    pub fn prepare(&self, request: BuildStorageReservation) -> Result<VolumeOperation> {
        if request.target != self.target {
            return Err(error("build_storage_target_conflict"));
        }
        let signed = SignedBuildStorageReservation::sign(request, &self.signer)
            .map_err(|_| error("invalid_build_storage_authority"))?;
        Ok(VolumeOperation {
            signed,
            request_uid: None,
        })
    }
    fn request_api(&self) -> Api<ConfigMap> {
        Api::namespaced(self.client.clone(), &self.requests_namespace)
    }
    fn reply_api(&self) -> Api<ConfigMap> {
        Api::namespaced(self.client.clone(), &self.receipts_namespace)
    }
    fn check_operation(&self, operation: &VolumeOperation) -> Result<String> {
        let request = &operation.signed.reservation;
        request
            .validate_shape()
            .map_err(|_| error("invalid_build_storage_authority"))?;
        if request.target != self.target {
            return Err(error("build_storage_target_conflict"));
        }
        request
            .queue_name()
            .map_err(|_| error("invalid_build_storage_authority"))
    }
    fn request_object(&self, operation: &VolumeOperation) -> Result<ConfigMap> {
        let r = &operation.signed.reservation;
        let bytes = serde_json::to_string(&operation.signed)
            .map_err(|_| error("invalid_build_storage_authority"))?;
        if bytes.len() > 8192 {
            return Err(error("build_storage_request_limit"));
        }
        serde_json::from_value(json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":self.check_operation(operation)?,"namespace":self.requests_namespace,
            "labels":{"webdock.dev/component":"build-volume-request","webdock.dev/volume-id":r.volume_id,"webdock.dev/generation":r.generation.to_string(),"webdock.dev/epoch":r.epoch.to_string()},
            "annotations":{"webdock.dev/request-sha256":r.fingerprint().map_err(|_|error("invalid_build_storage_authority"))?}},"immutable":true,"data":{"request.json":bytes}})).map_err(|_|error("invalid_build_storage_request"))
    }
    fn verify_request(&self, operation: &VolumeOperation, actual: &ConfigMap) -> Result<()> {
        let expected = self.request_object(operation)?;
        if actual.immutable != Some(true)
            || actual.metadata.name != expected.metadata.name
            || actual.metadata.namespace != expected.metadata.namespace
            || actual.metadata.uid.as_deref().is_none_or(str::is_empty)
            || actual.metadata.deletion_timestamp.is_some()
            || actual
                .metadata
                .resource_version
                .as_deref()
                .is_none_or(str::is_empty)
            || actual.metadata.labels != expected.metadata.labels
            || actual.data != expected.data
            || actual.binary_data.as_ref().is_some_and(|b| !b.is_empty())
            || actual
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get("webdock.dev/request-sha256"))
                != expected
                    .metadata
                    .annotations
                    .as_ref()
                    .and_then(|a| a.get("webdock.dev/request-sha256"))
            || operation
                .request_uid
                .as_ref()
                .is_some_and(|uid| Some(uid) != actual.metadata.uid.as_ref())
        {
            return Err(error("build_storage_request_conflict"));
        }
        Ok(())
    }
    async fn get(&self, api: &Api<ConfigMap>, name: &str) -> Result<Option<ConfigMap>> {
        tokio::time::timeout(Duration::from_secs(10), api.get_opt(name))
            .await
            .map_err(|_| error("build_storage_timeout"))?
            .map_err(|_| error("build_storage_unavailable"))
    }
    pub async fn send(&self, operation: &mut VolumeOperation) -> Result<()> {
        let name = self.check_operation(operation)?;
        let api = self.request_api();
        let actual = match self.get(&api, &name).await? {
            Some(actual) => actual,
            None => {
                operation
                    .signed
                    .verify(
                        &self.signer.verifying_key(),
                        &self.target,
                        chrono::Utc::now(),
                    )
                    .map_err(|_| error("build_storage_authority_expired"))?;
                if operation.request_uid.is_some() {
                    return Err(error("build_storage_request_lost"));
                }
                match tokio::time::timeout(
                    Duration::from_secs(10),
                    api.create(&PostParams::default(), &self.request_object(operation)?),
                )
                .await
                .map_err(|_| error("build_storage_timeout"))?
                {
                    Ok(value) => value,
                    Err(kube::Error::Api(e)) if e.code == 409 => self
                        .get(&api, &name)
                        .await?
                        .ok_or_else(|| error("build_storage_request_lost"))?,
                    Err(_) => return Err(error("build_storage_unavailable")),
                }
            }
        };
        self.verify_request(operation, &actual)?;
        operation.request_uid = actual.metadata.uid;
        Ok(())
    }
    fn verify_reply(
        &self,
        operation: &VolumeOperation,
        object: &ConfigMap,
    ) -> Result<BuildVolumeReply> {
        let name = self.check_operation(operation)?;
        let request = &operation.signed.reservation;
        if object.immutable != Some(true)
            || object.metadata.name.as_deref() != Some(name.as_str())
            || object.metadata.namespace.as_deref() != Some(self.receipts_namespace.as_str())
            || object.metadata.deletion_timestamp.is_some()
            || object.data.as_ref().is_none_or(|d| d.len() != 1)
            || object.binary_data.as_ref().is_some_and(|b| !b.is_empty())
        {
            return Err(error("build_storage_reply_conflict"));
        }
        let bytes = object
            .data
            .as_ref()
            .and_then(|d| d.get("reply.json"))
            .filter(|s| s.len() <= 16384)
            .ok_or_else(|| error("invalid_build_storage_reply"))?;
        let reply: BuildVolumeReply =
            serde_json::from_str(bytes).map_err(|_| error("invalid_build_storage_reply"))?;
        reply
            .validate_for(
                request,
                operation
                    .request_uid
                    .as_deref()
                    .ok_or_else(|| error("build_storage_request_unbound"))?,
                &self.execution_namespace,
            )
            .map_err(|_| error("build_storage_reply_conflict"))?;
        if object
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get("webdock.dev/request-sha256"))
            != Some(&reply.request_fingerprint)
        {
            return Err(error("build_storage_reply_conflict"));
        }
        Ok(reply)
    }
    pub async fn reply(&self, operation: &VolumeOperation) -> Result<Option<BuildVolumeReply>> {
        let name = self.check_operation(operation)?;
        self.get(&self.reply_api(), &name)
            .await?
            .map(|r| self.verify_reply(operation, &r))
            .transpose()
    }
    async fn delete(&self, api: &Api<ConfigMap>, object: &ConfigMap) -> Result<()> {
        let params = DeleteParams {
            preconditions: Some(Preconditions {
                uid: Some(
                    object
                        .metadata
                        .uid
                        .clone()
                        .ok_or_else(|| error("build_storage_resource_unbound"))?,
                ),
                resource_version: Some(
                    object
                        .metadata
                        .resource_version
                        .clone()
                        .ok_or_else(|| error("build_storage_resource_unbound"))?,
                ),
            }),
            ..Default::default()
        };
        match tokio::time::timeout(
            Duration::from_secs(10),
            api.delete(
                object
                    .metadata
                    .name
                    .as_deref()
                    .ok_or_else(|| error("invalid_build_storage_resource"))?,
                &params,
            ),
        )
        .await
        .map_err(|_| error("build_storage_timeout"))?
        {
            Ok(_) => Ok(()),
            Err(kube::Error::Api(e)) if e.code == 404 => Ok(()),
            Err(_) => Err(error("build_storage_unavailable")),
        }
    }
    pub async fn cleanup(&self, operation: &VolumeOperation) -> Result<()> {
        let name = self.check_operation(operation)?;
        let mut bound = operation.clone();
        if let Some(request) = self.get(&self.request_api(), &name).await? {
            self.verify_request(operation, &request)?;
            bound.request_uid = request.metadata.uid.clone();
            self.delete(&self.request_api(), &request).await?;
        }
        if self.get(&self.request_api(), &name).await?.is_some() {
            return Err(error("build_storage_request_retiring"));
        }
        if let Some(reply) = self.get(&self.reply_api(), &name).await? {
            if bound.request_uid.is_none() {
                let bytes = reply
                    .data
                    .as_ref()
                    .and_then(|d| d.get("reply.json"))
                    .filter(|s| s.len() <= 16384)
                    .ok_or_else(|| error("invalid_build_storage_reply"))?;
                let value: BuildVolumeReply = serde_json::from_str(bytes)
                    .map_err(|_| error("invalid_build_storage_reply"))?;
                bound.request_uid = Some(value.request_uid);
            }
            // When create's response was lost, the protected node receipt supplies
            // its request UID; the full original operation fingerprint still binds it.
            self.verify_reply(&bound, &reply)?;
            self.delete(&self.reply_api(), &reply).await?;
        }
        Ok(())
    }
}
