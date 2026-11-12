//! Router is mounted by Core at the retained /api/hosting/git/worker prefix.
use crate::{BuildService, Result, artifact::StoredArtifact, error};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Artifact {
    kind: String,
    digest: String,
    storage_key: String,
    size_bytes: u64,
    image_digest: String,
    #[serde(default)]
    remote_storage: Option<webdock_object_storage::ObjectReceipt>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Completion {
    #[serde(rename = "buildID")]
    build_id: String,
    generation: i32,
    status: String,
    logs: String,
    #[serde(default)]
    failure_code: Option<String>,
    #[serde(default)]
    artifact: Option<Artifact>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Release {
    #[serde(rename = "releaseID")]
    release_id: String,
    generation: i32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseClaim {
    #[serde(default)]
    artifact_store_bindings: Vec<String>,
    #[serde(default)]
    publication_projects: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default = "legacy_formats")]
    artifact_formats: Vec<u8>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BuildClaim {
    #[serde(default = "legacy_recipes")]
    recipes: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KubernetesClaim {
    #[serde(rename = "clusterID")]
    cluster_id: String,
    runtime_fingerprint: String,
    #[serde(default)]
    storage_profile: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KubernetesEnvironment {
    #[serde(rename = "buildID")]
    build_id: String,
    generation: i32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KubernetesStorageIssue {
    #[serde(rename = "buildID")]
    build_id: String,
    generation: i32,
    action: webdock_agent_protocol::build_storage::BuildStorageAction,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KubernetesVolumeBinding {
    #[serde(rename = "buildID")]
    build_id: String,
    generation: i32,
    handle: webdock_agent_protocol::build_storage::BuildVolumeHandle,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KubernetesVolumeRetired {
    #[serde(rename = "buildID")]
    build_id: String,
    generation: i32,
    epoch: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KubernetesCluster {
    #[serde(rename = "clusterID")]
    cluster_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KubernetesIntent {
    #[serde(rename = "buildID")]
    build_id: String,
    generation: i32,
    spec_fingerprint: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KubernetesJob {
    #[serde(rename = "buildID")]
    build_id: String,
    generation: i32,
    uid: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KubernetesRetire {
    #[serde(rename = "buildID")]
    build_id: String,
    generation: i32,
    uid: Option<String>,
    infrastructure_failure: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KubernetesLog {
    #[serde(rename = "buildID")]
    build_id: String,
    generation: i32,
    entry_key: String,
    phase: String,
    text: String,
}
fn legacy_recipes() -> Vec<String> {
    vec!["dockerfile".into()]
}
fn legacy_formats() -> Vec<u8> {
    vec![0]
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Publish {
    #[serde(rename = "releaseID")]
    release_id: String,
    generation: i32,
    image: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseComplete {
    #[serde(rename = "releaseID")]
    release_id: String,
    generation: i32,
    status: String,
    #[serde(default, rename = "operationID")]
    operation_id: Option<String>,
    #[serde(default)]
    health_verified: Option<bool>,
}
fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|_| error("invalid_command"))
}
fn lease(id: &str, generation: i32) -> Result<()> {
    crate::contracts::id(id)?;
    if generation <= 0 {
        return Err(error("invalid_generation"));
    }
    Ok(())
}
fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
pub fn worker_router(service: Arc<BuildService>) -> Router {
    if !service.execution_configured() {
        return Router::new();
    }
    Router::new()
        .route("/{*path}", any(handler))
        .with_state(service)
}
fn response(value: Value, status: u16) -> Response {
    let mut r = (
        StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
        axum::Json(value),
    )
        .into_response();
    r.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    r
}
async fn handler(State(service): State<Arc<BuildService>>, request: Request) -> Response {
    match dispatch(&service, request).await {
        Ok(response) => response,
        Err(e) => response(json!({"error":e.code}), e.status()),
    }
}
async fn dispatch(service: &BuildService, request: Request) -> Result<Response> {
    let auth = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| {
            v.len() == 43
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        })
        .ok_or_else(|| error("worker_authentication_failed"))?;
    let worker = service.authenticate_worker(auth).await?;
    let path = request
        .uri()
        .path()
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    if request.method() == "GET" {
        if path.len() != 3 || !matches!(path[0], "builds" | "releases") {
            return Err(error("endpoint_unavailable"));
        }
        let parameters =
            url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
                .collect::<Vec<_>>();
        if parameters.len() != 1
            || parameters[0].0 != "generation"
            || parameters[0].1.starts_with('0')
            || !parameters[0].1.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(error("invalid_generation"));
        }
        let generation = parameters[0]
            .1
            .parse::<i32>()
            .map_err(|_| error("invalid_generation"))?;
        lease(path[1], generation)?;
        match (path[0], path[2]) {
            ("builds", "lease") => service.guard_build(&worker, path[1], generation).await?,
            ("releases", "lease") => service.guard_release(&worker, path[1], generation).await?,
            ("builds", "archive") => {
                let bytes = service.source_archive(&worker, path[1], generation).await?;
                if bytes.len() as u64 > crate::artifact::MAX_SOURCE {
                    return Err(error("source_limit"));
                }
                let mut r = Response::new(Body::from(bytes));
                r.headers_mut().insert(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static("application/x-tar"),
                );
                r.headers_mut().insert(
                    header::CACHE_CONTROL,
                    header::HeaderValue::from_static("no-store"),
                );
                return Ok(r);
            }
            _ => return Err(error("endpoint_unavailable")),
        }
        return Ok(response(json!({"active":true}), 200));
    }
    if request.method() != "POST" || path.len() != 1 || request.uri().query().is_some() {
        return Err(error("endpoint_unavailable"));
    }
    let endpoint = path[0].to_owned();
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|v| v.split(';').next() != Some("application/json"))
    {
        return Err(error("invalid_content_type"));
    }
    let bytes = to_bytes(request.into_body(), 400000)
        .await
        .map_err(|_| error("request_limit"))?;
    let input: Value = serde_json::from_slice(&bytes).map_err(|_| error("invalid_json"))?;
    let output = match endpoint.as_str() {
        "capabilities" => {
            if input != json!({}) {
                return Err(error("invalid_command"));
            }
            json!({"artifactFormats":crate::artifact::SUPPORTED_FORMATS,"buildRecipes":["dockerfile",crate::frontend::RECIPE]})
        }
        "claim" => {
            let claim: BuildClaim = parse(input)?;
            service
                .claim_build_compatible(&worker, &claim.recipes)
                .await?
                .map(|mut job| {
                    let request = job["request"].clone();
                    if let Some(object) = request.as_object() {
                        for (k, v) in object {
                            job[k] = v.clone();
                        }
                    }
                    job["buildProvider"] = json!("isolated");
                    job
                })
                .unwrap_or(Value::Null)
        }
        "kubernetes-log" => {
            let value: KubernetesLog = parse(input)?;
            lease(&value.build_id, value.generation)?;
            service
                .append_kubernetes_log(
                    &worker,
                    &value.build_id,
                    value.generation,
                    &value.entry_key,
                    &value.phase,
                    &value.text,
                )
                .await?
        }
        "kubernetes-claim" => {
            let value: KubernetesClaim = parse(input)?;
            if value
                .storage_profile
                .as_deref()
                .is_some_and(|p| p != "fixed-ext4-v1")
            {
                return Err(error("invalid_build_storage_profile"));
            }
            service
                .claim_kubernetes_build_with_storage(
                    &worker,
                    &value.cluster_id,
                    &value.runtime_fingerprint,
                    value.storage_profile.is_some(),
                )
                .await?
                .unwrap_or(Value::Null)
        }
        "kubernetes-environment" => {
            let value: KubernetesEnvironment = parse(input)?;
            lease(&value.build_id, value.generation)?;
            serde_json::to_value(
                service
                    .kubernetes_build_environment(&worker, &value.build_id, value.generation)
                    .await?,
            )
            .map_err(|_| error("invalid_build_environment"))?
        }
        "kubernetes-storage" => {
            let value: KubernetesStorageIssue = parse(input)?;
            lease(&value.build_id, value.generation)?;
            serde_json::to_value(
                service
                    .issue_kubernetes_storage(
                        &worker,
                        &value.build_id,
                        value.generation,
                        value.action,
                    )
                    .await?,
            )
            .map_err(|_| error("invalid_build_storage_authority"))?
        }
        "kubernetes-volume" => {
            let value: KubernetesVolumeBinding = parse(input)?;
            lease(&value.build_id, value.generation)?;
            service
                .record_kubernetes_volume(&worker, &value.build_id, value.generation, value.handle)
                .await?;
            json!({"recorded":true})
        }
        "kubernetes-volume-retired" => {
            let value: KubernetesVolumeRetired = parse(input)?;
            lease(&value.build_id, value.generation)?;
            service
                .record_kubernetes_volume_retired(
                    &worker,
                    &value.build_id,
                    value.generation,
                    value.epoch,
                )
                .await?;
            json!({"retired":true})
        }
        "kubernetes-attempts" => {
            let value: KubernetesCluster = parse(input)?;
            crate::contracts::id(&value.cluster_id)?;
            json!({"attempts":service.list_kubernetes_attempts(&worker, &value.cluster_id).await?})
        }
        "kubernetes-intent" => {
            let value: KubernetesIntent = parse(input)?;
            lease(&value.build_id, value.generation)?;
            service
                .record_kubernetes_job_intent(
                    &worker,
                    &value.build_id,
                    value.generation,
                    &value.spec_fingerprint,
                )
                .await?;
            json!({"recorded":true})
        }
        "kubernetes-job" => {
            let value: KubernetesJob = parse(input)?;
            lease(&value.build_id, value.generation)?;
            service
                .record_kubernetes_job(&worker, &value.build_id, value.generation, &value.uid)
                .await?;
            json!({"recorded":true})
        }
        "kubernetes-retire" => {
            let value: KubernetesRetire = parse(input)?;
            lease(&value.build_id, value.generation)?;
            if value.infrastructure_failure.as_deref().is_some_and(|code| {
                !matches!(
                    code,
                    "KUBERNETES_API_UNAVAILABLE" | "KUBERNETES_NODE_UNAVAILABLE"
                )
            }) {
                return Err(error("kubernetes_failure_not_retryable"));
            }
            let retry = service
                .retire_kubernetes_attempt_with_retry(
                    &worker,
                    &value.build_id,
                    value.generation,
                    value.uid.as_deref(),
                    value.infrastructure_failure.as_deref(),
                )
                .await?;
            json!({"retired":true,"retryScheduled":retry})
        }
        "complete" => {
            let value: Completion = parse(input)?;
            lease(&value.build_id, value.generation)?;
            if !matches!(value.status.as_str(), "succeeded" | "failed")
                || value.logs.len() > 262144
                || value.status == "succeeded" && value.artifact.is_none()
                || value.status == "failed" && value.artifact.is_some()
            {
                return Err(error("invalid_completion"));
            }
            let artifact = value
                .artifact
                .map(|a| {
                    if a.kind != "oci"
                        || !digest(&a.digest)
                        || !digest(&a.image_digest)
                        || a.size_bytes == 0
                        || a.size_bytes > crate::artifact::MAX_OUTPUT
                        || a.storage_key.len() > 240
                    {
                        return Err(error("invalid_artifact"));
                    }
                    let p = a.storage_key.split('/').collect::<Vec<_>>();
                    if p.len() != 4 {
                        return Err(error("invalid_artifact"));
                    }
                    for id in &p[..3] {
                        crate::contracts::id(id)?;
                    }
                    if p[1] != value.build_id || p[2] != value.generation.to_string() {
                        return Err(error("artifact_scope_mismatch"));
                    }
                    crate::artifact::storage_format(&a.storage_key, &a.digest)?;
                    Ok(StoredArtifact {
                        key: a.storage_key,
                        digest: a.digest,
                        size: a.size_bytes,
                        image_digest: a.image_digest,
                        remote: a.remote_storage,
                    })
                })
                .transpose()?;
            let failure = if value.status == "failed" {
                Some(value.failure_code.as_deref().unwrap_or("BUILD_FAILED"))
            } else {
                if value.failure_code.is_some() {
                    return Err(error("invalid_completion"));
                }
                None
            };
            service
                .complete_build(
                    &worker,
                    &value.build_id,
                    value.generation,
                    artifact.as_ref(),
                    &value.logs,
                    failure,
                )
                .await?
        }
        "release-claim" => {
            let claim: ReleaseClaim = parse(input)?;
            if claim.publication_projects.is_none() && !claim.artifact_store_bindings.is_empty() {
                return Err(error("invalid_publication_projects"));
            }
            let release = service
                .claim_release_compatible(
                    &worker,
                    &claim.artifact_store_bindings,
                    claim.publication_projects.as_ref(),
                    &claim.artifact_formats,
                )
                .await?;
            release.map(|r|json!({"releaseID":r["id"],"customerID":r["customer_id"],"projectID":r["project_id"],"buildID":r["build_id"],"generation":r["generation"],"leaseUntil":r["lease_until"],"artifact":{"kind":"oci","digest":r["digest"],"storageKey":r["storage_key"],"sizeBytes":r["size_bytes"],"imageDigest":r["image_digest"],"remoteStorage":r["remote_storage"]}})).unwrap_or(Value::Null)
        }
        "release-prepare" => {
            let r: Release = parse(input)?;
            lease(&r.release_id, r.generation)?;
            let mut result = service
                .prepare_publication(&worker, &r.release_id, r.generation)
                .await?;
            result["kind"] = json!("oci");
            result
        }
        "container-publish" => {
            let r: Publish = parse(input)?;
            lease(&r.release_id, r.generation)?;
            let result = service
                .publish_image(&worker, &r.release_id, r.generation, &r.image)
                .await?;
            json!({"operationID":result["id"],"status":"deploying"})
        }
        "observe" => {
            let r: Release = parse(input)?;
            lease(&r.release_id, r.generation)?;
            service
                .observe_release(&worker, &r.release_id, r.generation)
                .await?
        }
        "release-complete" => {
            let r: ReleaseComplete = parse(input)?;
            lease(&r.release_id, r.generation)?;
            match r.status.as_str() {
                "needs-reconciliation" | "failed" => {
                    service
                        .publication_uncertain(&worker, &r.release_id, r.generation)
                        .await?;
                    json!({"id":r.release_id,"status":"needs-reconciliation"})
                }
                "ready" | "deploying" => {
                    let result = service
                        .observe_release(&worker, &r.release_id, r.generation)
                        .await?;
                    if r.operation_id
                        .as_ref()
                        .is_some_and(|id| result["operationID"] != *id)
                        || r.status == "ready"
                            && (result["status"] != "ready" || r.health_verified != Some(true))
                    {
                        return Err(error("publication_proof_conflict"));
                    }
                    result
                }
                _ => return Err(error("invalid_release_status")),
            }
        }
        _ => return Err(error("endpoint_unavailable")),
    };
    Ok(response(output, 200))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_wire_claims_keep_only_legacy_capabilities() {
        let build: BuildClaim = parse(json!({})).unwrap();
        assert_eq!(build.recipes, ["dockerfile"]);
        let release: ReleaseClaim = parse(json!({})).unwrap();
        assert_eq!(release.artifact_formats, [0]);
        let build: BuildClaim = parse(json!({"recipes":["dockerfile-frontend"]})).unwrap();
        assert_eq!(build.recipes, ["dockerfile-frontend"]);
        let release: ReleaseClaim = parse(json!({"artifactFormats":[0,1,2]})).unwrap();
        assert_eq!(release.artifact_formats, [0, 1, 2]);
        assert!(parse::<BuildClaim>(json!({"recipe":"dockerfile-frontend"})).is_err());
        assert!(parse::<ReleaseClaim>(json!({"artifactFormat":2})).is_err());
    }
}
