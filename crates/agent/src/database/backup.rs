//! Native CNPG backup objects; Secret values exist only at the authenticated material boundary.
use super::executor::*;
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use webdock_agent_protocol::database::{
    DatabaseBackupControls, DatabaseBackupProof, DatabaseBackupResources, DatabaseCheckpointPhase,
    DatabaseClaim, DatabaseCompletionProof, DatabaseExecutionPhase, DatabaseObjectStoreMaterial,
    DatabaseProofState,
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseBackupRuntimeConfig {
    pub plugin_image: String,
    pub sidecar_image: String,
    pub endpoints: Vec<String>,
    #[serde(rename = "egressCIDRs")]
    pub egress_cidrs: Vec<String>,
    /// Public trust anchor for explicitly approved private HTTPS endpoints.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "endpointCACertificate"
    )]
    pub endpoint_ca_certificate: Option<String>,
}
impl DatabaseBackupRuntimeConfig {
    pub fn validate(&self) -> Result<()> {
        if let Some(pem) = &self.endpoint_ca_certificate {
            if pem.len() > 16384 || pem.contains("PRIVATE KEY") {
                return Err(invalid());
            }
            let certificates = rustls_pemfile::certs(&mut pem.as_bytes())
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|_| invalid())?;
            if certificates.is_empty()
                || certificates
                    .iter()
                    .any(|der| x509_parser::parse_x509_certificate(der).is_err())
            {
                return Err(invalid());
            }
        }
        for image in [&self.plugin_image, &self.sidecar_image] {
            let image: oci_spec::distribution::Reference = image.parse().map_err(|_| invalid())?;
            if image.digest().is_none_or(|d| {
                !d.strip_prefix("sha256:").is_some_and(|d| {
                    d.len() == 64
                        && d.bytes()
                            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                })
            }) {
                return Err(invalid());
            }
        }
        if self.endpoints.is_empty()
            || self.endpoints.len() > 10
            || self.egress_cidrs.is_empty()
            || self.egress_cidrs.len() > 20
        {
            return Err(invalid());
        }
        for cidr in &self.egress_cidrs {
            let network: ipnet::IpNet = cidr.parse().map_err(|_| invalid())?;
            if network != network.trunc() || network.prefix_len() == 0 {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
async fn latest_scheduled_backup(
    client: &kube::Client,
    namespace: &str,
    schedule_uid: &str,
) -> Result<Option<kube::api::DynamicObject>> {
    let resource = kube::api::ApiResource::from_gvk(&kube::api::GroupVersionKind::gvk(
        "postgresql.cnpg.io",
        "v1",
        "Backup",
    ));
    let api: kube::Api<kube::api::DynamicObject> =
        kube::Api::namespaced_with(client.clone(), namespace, &resource);
    let mut continuation = String::new();
    let mut snapshot = None;
    let mut latest: Option<kube::api::DynamicObject> = None;
    loop {
        let params = kube::api::ListParams::default()
            .limit(100)
            .continue_token(&continuation);
        let list = api
            .list(&params)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if let Some(previous) = &snapshot {
            if list.metadata.resource_version.as_ref() != Some(previous) {
                return Err(ExecutionError::Conflict);
            }
        } else {
            snapshot = list.metadata.resource_version.clone();
        }
        for item in list.items {
            if item.data["status"]["phase"] != "completed"
                || item.data["spec"]["cluster"]["name"] != "postgres"
                || !item
                    .metadata
                    .owner_references
                    .as_ref()
                    .is_some_and(|refs| refs.iter().any(|owner| owner.uid == schedule_uid))
            {
                continue;
            }
            let time = item.data["status"]["stoppedAt"]
                .as_str()
                .and_then(|v| v.parse::<chrono::DateTime<chrono::Utc>>().ok())
                .ok_or(ExecutionError::Runtime)?;
            let replace = latest.as_ref().is_none_or(|v| {
                v.data["status"]["stoppedAt"]
                    .as_str()
                    .and_then(|v| v.parse::<chrono::DateTime<chrono::Utc>>().ok())
                    .is_some_and(|old| time > old)
            });
            if replace {
                latest = Some(item);
            }
        }
        let next = list.metadata.continue_.unwrap_or_default();
        if next.is_empty() {
            return Ok(latest);
        }
        if next == continuation {
            return Err(ExecutionError::Conflict);
        }
        continuation = next;
    }
}

fn quantities(r: &DatabaseBackupResources) -> Value {
    json!({"cpu":format!("{}m",r.cpu_millicores),"memory":r.memory_bytes.to_string(),"ephemeral-storage":r.ephemeral_bytes.to_string()})
}
/// Includes a conservative concurrent recovery-job peak as well as the restartable init sidecar.
pub fn backup_manifests(
    job: &DatabaseJobSpec,
    config: &DatabaseRuntimeConfig,
    material: &DatabaseObjectStoreMaterial,
) -> Result<Vec<Value>> {
    config.admit(job)?;
    let recovery = job.recovery.is_some();
    if material.read_only != recovery {
        return Err(ExecutionError::Denied);
    }
    let p = job
        .backup
        .as_ref()
        .or_else(|| job.recovery.as_ref().map(|r| &r.source))
        .ok_or(ExecutionError::Denied)?;
    let d = &p.destination;
    let b = &job.backend;
    let labels = json!({"webdock.dev/database-installation":b.id,"webdock.dev/backup-policy":p.id});
    let meta = |name: &str| json!({"name":name,"namespace":b.namespace,"labels":labels,"annotations":{"webdock.dev/backup-policy-revision":p.revision.to_string()}});
    let (credential, revision) = if recovery {
        (&d.recovery_credential_id, d.recovery_credential_revision)
    } else {
        (&d.credential_id, d.credential_revision)
    };
    let secret = format!("s3-{credential}-{revision}");
    let store = format!("backup-{}", p.id);
    let mut object_store = json!({"apiVersion":"barmancloud.cnpg.io/v1","kind":"ObjectStore","metadata":meta(&store),"spec":{"configuration":{"destinationPath":format!("s3://{}/{}/",d.bucket,d.archive_path),"endpointURL":d.endpoint_url,"s3Credentials":{"accessKeyId":{"name":secret,"key":"ACCESS_KEY_ID"},"secretAccessKey":{"name":secret,"key":"ACCESS_SECRET_KEY"}},"wal":{"compression":"gzip","maxParallel":2},"data":{"compression":"gzip","jobs":2}},"instanceSidecarConfiguration":{"resources":{"requests":quantities(&p.sidecar),"limits":quantities(&p.sidecar)},"env":[{"name":"AWS_DEFAULT_REGION","value":d.region}]}}});
    object_store["spec"]["retentionPolicy"] = if p.retention_delete && !recovery {
        json!(format!("{}d", p.retention_days))
    } else {
        Value::Null
    };
    let peak_cpu =
        b.resources.cpu_millicores + 200 + p.sidecar.cpu_millicores + p.recovery_job.cpu_millicores;
    let peak_memory =
        b.resources.memory_bytes + 536870912 + p.sidecar.memory_bytes + p.recovery_job.memory_bytes;
    let peak_ephemeral = 536870912 + p.sidecar.ephemeral_bytes + p.recovery_job.ephemeral_bytes;
    let runtime = config.backup.as_ref().ok_or(ExecutionError::Denied)?;
    let endpoint_port = d
        .endpoint_url
        .trim_end_matches('/')
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse::<u16>().ok())
        .unwrap_or(443);
    let mut result = vec![
        json!({"apiVersion":"v1","kind":"ResourceQuota","metadata":meta("database-budget"),"spec":{"hard":{"requests.cpu":format!("{peak_cpu}m"),"limits.cpu":format!("{peak_cpu}m"),"requests.memory":peak_memory.to_string(),"limits.memory":peak_memory.to_string(),"requests.ephemeral-storage":peak_ephemeral.to_string(),"limits.ephemeral-storage":peak_ephemeral.to_string(),"requests.storage":b.resources.volume_bytes.to_string(),"persistentvolumeclaims":"1","pods":"4"}}}),
        json!({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":meta("backup-egress"),"spec":{"podSelector":{},"policyTypes":["Egress"],"egress":[{"to":runtime.egress_cidrs.iter().map(|cidr|json!({"ipBlock":{"cidr":cidr}})).collect::<Vec<_>>(),"ports":[{"protocol":"TCP","port":endpoint_port}]}]}}),
        json!({"apiVersion":"v1","kind":"Secret","metadata":meta(&secret),"type":"Opaque","immutable":true,"data":{"ACCESS_KEY_ID":STANDARD.encode(&material.access_key_id),"ACCESS_SECRET_KEY":STANDARD.encode(&material.secret_access_key)}}),
        object_store,
    ];
    if let Some(certificate) = &runtime.endpoint_ca_certificate {
        let name = format!("s3-ca-{}", d.id);
        result[3]["spec"]["configuration"]["endpointCA"] = json!({"name":name,"key":"ca.crt"});
        result.insert(3, json!({"apiVersion":"v1","kind":"Secret","metadata":meta(&name),"type":"Opaque","immutable":true,"data":{"ca.crt":STANDARD.encode(certificate)}}));
    }
    if !recovery {
        result.push(json!({"apiVersion":"postgresql.cnpg.io/v1","kind":"ScheduledBackup","metadata":meta(&store),"spec":{"cluster":{"name":"postgres"},"method":"plugin","pluginConfiguration":{"name":"barman-cloud.cloudnative-pg.io"},"schedule":p.schedule,"suspend":p.suspended,"immediate":false,"backupOwnerReference":"self"}}));
        result.push(json!({"apiVersion":"postgresql.cnpg.io/v1","kind":"Cluster","metadata":meta("postgres"),"spec":{"plugins":[{"name":"barman-cloud.cloudnative-pg.io","isWALArchiver":p.wal_enabled,"parameters":{"barmanObjectName":store,"serverName":d.server_name}}]}}));
    }
    Ok(result)
}
impl NativeDatabaseExecutor {
    pub(super) async fn recovery_digest(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<String> {
        let r = claim.spec.recovery.as_ref().ok_or(ExecutionError::Denied)?;
        let d = claim.spec.database.as_ref().ok_or(ExecutionError::Denied)?;
        let digest = self
            .sql(claim, authority, &d.name, include_str!("backup-digest.sql"))
            .await?;
        if digest != r.verification_sha256 {
            return Err(ExecutionError::Ownership);
        }
        Ok(digest)
    }
    pub(super) async fn configure_recovery(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<()> {
        let r = claim.spec.recovery.as_ref().ok_or(ExecutionError::Denied)?;
        let d = claim.spec.database.as_ref().ok_or(ExecutionError::Denied)?;
        if let Some(old_export) = super::exports::ExportRole::new(
            &r.source_database_id,
            &format!("db_{}", r.source_database_id),
            "dedicated",
        )
        .map_err(|_| ExecutionError::Ownership)?
        {
            self.sql(claim, authority, "postgres", &old_export.disable)
                .await?;
        }
        if !self.database_exists(claim, authority).await? {
            let source = &r.source_database_id;
            let marker=self.sql(claim,authority,"postgres",&format!("SELECT pg_catalog.shobj_description(oid,'pg_database') FROM pg_catalog.pg_database WHERE datname='db_{source}';")).await?;
            if marker != format!("webdock:database:{source}") {
                return Err(ExecutionError::Ownership);
            }
            // Rename preserves restored ownership, grants, SCRAM verifiers and all other roles.
            let mut sql = format!(
                "ALTER DATABASE db_{source} WITH ALLOW_CONNECTIONS false; SELECT pg_catalog.pg_terminate_backend(pid) FROM pg_catalog.pg_stat_activity WHERE datname='db_{source}'; ALTER DATABASE db_{source} RENAME TO {}; COMMENT ON DATABASE {} IS 'webdock:database:{}'; ALTER DATABASE {} WITH ALLOW_CONNECTIONS true;",
                d.name, d.name, d.id, d.name
            );
            for prefix in ["wd_owner_", "wd_migrate_", "wd_app_"] {
                sql.push_str(&format!("DO $webdock$ BEGIN IF (SELECT pg_catalog.shobj_description(oid,'pg_authid') FROM pg_catalog.pg_roles WHERE rolname='{prefix}{source}') IS DISTINCT FROM 'webdock:database:{source}' THEN RAISE EXCEPTION 'Recovery role conflict'; END IF; END $webdock$; ALTER ROLE {prefix}{source} RENAME TO {prefix}{}; COMMENT ON ROLE {prefix}{} IS 'webdock:database:{}';",d.id,d.id,d.id));
            }
            self.sql(claim, authority, "postgres", &sql).await?;
        }
        self.sql(
            claim,
            authority,
            "postgres",
            &super::sql::statements(&claim.spec)?.enable,
        )
        .await?;
        self.recovery_digest(claim, authority).await?;
        // Credentials are not reset: restored verifiers must match the recovered Core version.
        Ok(())
    }
    pub(super) async fn verify_backup_runtime(&self) -> Result<()> {
        let config = self.config.backup.as_ref().ok_or(ExecutionError::Denied)?;
        if !self.admission_ready("webdock-database-backup-pods").await? {
            return Err(ExecutionError::Denied);
        }
        let deployment = self
            .get("Deployment", "barman-cloud", Some("cnpg-system"))
            .await?
            .ok_or(ExecutionError::Denied)?;
        if deployment.data["status"]["availableReplicas"] != 1 {
            return Err(ExecutionError::Denied);
        }
        let containers = deployment.data["spec"]["template"]["spec"]["containers"]
            .as_array()
            .ok_or(ExecutionError::Denied)?;
        let manager = containers
            .iter()
            .find(|c| c["image"] == config.plugin_image)
            .ok_or(ExecutionError::Denied)?;
        let env = manager["env"]
            .as_array()
            .and_then(|env| env.iter().find(|e| e["name"] == "SIDECAR_IMAGE"))
            .ok_or(ExecutionError::Denied)?;
        let image = if let Some(value) = env["value"].as_str() {
            value.to_owned()
        } else {
            let name = env["valueFrom"]["secretKeyRef"]["name"]
                .as_str()
                .ok_or(ExecutionError::Denied)?;
            let key = env["valueFrom"]["secretKeyRef"]["key"]
                .as_str()
                .ok_or(ExecutionError::Denied)?;
            let secret = self
                .get("Secret", name, Some("cnpg-system"))
                .await?
                .ok_or(ExecutionError::Denied)?;
            let encoded = secret.data["data"][key]
                .as_str()
                .ok_or(ExecutionError::Denied)?;
            String::from_utf8(
                STANDARD
                    .decode(encoded)
                    .map_err(|_| ExecutionError::Denied)?,
            )
            .map_err(|_| ExecutionError::Denied)?
        };
        if image != config.sidecar_image {
            return Err(ExecutionError::Denied);
        }
        Ok(())
    }
    pub(super) async fn backup_stage(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<DatabaseStep> {
        let job = &claim.spec;
        let p = job.backup.as_ref().ok_or(ExecutionError::Denied)?;
        let b = &job.backend;
        let cluster = self
            .get("Cluster", "postgres", Some(&b.namespace))
            .await?
            .ok_or(ExecutionError::Ownership)?;
        self.owned(&cluster, job)?;
        if cluster.metadata.uid.as_ref() != Some(&p.source_cnpg_uid) {
            return Err(ExecutionError::Ownership);
        }
        self.verify_storage(claim, authority).await?;
        let objects = backup_manifests(
            job,
            &self.config,
            claim
                .material
                .object_store
                .as_ref()
                .ok_or(ExecutionError::Denied)?,
        )?;
        match job.phase {
            DatabaseExecutionPhase::Apply => {
                for o in objects {
                    self.apply(o, claim, authority).await?;
                }
                self.checkpoint(
                    claim,
                    authority,
                    DatabaseCheckpointPhase::Applied,
                    &p.source_cnpg_uid,
                )
                .await?;
                Ok(DatabaseStep::Pending)
            }
            DatabaseExecutionPhase::Configure => {
                self.checkpoint(
                    claim,
                    authority,
                    DatabaseCheckpointPhase::Configuring,
                    &p.source_cnpg_uid,
                )
                .await?;
                if job.action == DatabaseAction::Backup {
                    self.apply(json!({"apiVersion":"postgresql.cnpg.io/v1","kind":"Backup","metadata":{"name":format!("backup-{}",job.operation_id),"namespace":b.namespace,"labels":{"webdock.dev/database-installation":b.id,"webdock.dev/backup-policy":p.id},"annotations":{"webdock.dev/backup-policy-revision":p.revision.to_string()}},"spec":{"cluster":{"name":"postgres"},"method":"plugin","pluginConfiguration":{"name":"barman-cloud.cloudnative-pg.io"}}}),claim,authority).await?;
                }
                self.checkpoint(
                    claim,
                    authority,
                    DatabaseCheckpointPhase::Observing,
                    &p.source_cnpg_uid,
                )
                .await?;
                Ok(DatabaseStep::Pending)
            }
            DatabaseExecutionPhase::Observe => {
                let schedule = self
                    .get(
                        "ScheduledBackup",
                        &format!("backup-{}", p.id),
                        Some(&b.namespace),
                    )
                    .await?
                    .ok_or(ExecutionError::Ownership)?;
                self.owned(&schedule, job)?;
                if schedule.data["spec"]["suspend"] != p.suspended
                    || schedule.data["spec"]["schedule"] != p.schedule
                    || schedule
                        .metadata
                        .annotations
                        .as_ref()
                        .and_then(|a| a.get("webdock.dev/backup-policy-revision"))
                        != Some(&p.revision.to_string())
                {
                    return Ok(DatabaseStep::Pending);
                }
                let store = self
                    .get(
                        "ObjectStore",
                        &format!("backup-{}", p.id),
                        Some(&b.namespace),
                    )
                    .await?
                    .ok_or(ExecutionError::Ownership)?;
                self.owned(&store, job)?;
                let current = self
                    .get("Cluster", "postgres", Some(&b.namespace))
                    .await?
                    .ok_or(ExecutionError::Ownership)?;
                self.owned(&current, job)?;
                let desired_store = backup_manifests(
                    job,
                    &self.config,
                    claim
                        .material
                        .object_store
                        .as_ref()
                        .ok_or(ExecutionError::Denied)?,
                )?
                .into_iter()
                .find(|o| o["kind"] == "ObjectStore")
                .ok_or(ExecutionError::Runtime)?;
                if store.data["spec"]["configuration"] != desired_store["spec"]["configuration"]
                    || store.data["spec"]["retentionPolicy"]
                        != desired_store["spec"]["retentionPolicy"]
                    || !current.data["spec"]["plugins"]
                        .as_array()
                        .is_some_and(|plugins| {
                            plugins.iter().any(|plugin| {
                                plugin["name"] == "barman-cloud.cloudnative-pg.io"
                                    && plugin["isWALArchiver"] == p.wal_enabled
                                    && plugin["parameters"]["serverName"]
                                        == p.destination.server_name
                                    && plugin["parameters"]["barmanObjectName"]
                                        == format!("backup-{}", p.id)
                            })
                        })
                {
                    return Ok(DatabaseStep::Pending);
                }
                let mut proof = DatabaseBackupProof {
                    policy_id: p.id.clone(),
                    revision: p.revision,
                    scheduled_backup_uid: schedule.metadata.uid.ok_or(ExecutionError::Ownership)?,
                    suspended: p.suspended,
                    controls: DatabaseBackupControls {
                        schedule: schedule.data["spec"]["schedule"]
                            .as_str()
                            .ok_or(ExecutionError::Runtime)?
                            .into(),
                        wal_enabled: p.wal_enabled,
                        retention_delete: !store.data["spec"]["retentionPolicy"].is_null(),
                        retention_days: p.retention_days,
                        scheduled_backup_resource_version: schedule
                            .metadata
                            .resource_version
                            .ok_or(ExecutionError::Ownership)?,
                        cluster_resource_version: current
                            .metadata
                            .resource_version
                            .ok_or(ExecutionError::Ownership)?,
                        object_store_uid: store.metadata.uid.ok_or(ExecutionError::Ownership)?,
                        object_store_resource_version: store
                            .metadata
                            .resource_version
                            .ok_or(ExecutionError::Ownership)?,
                    },
                    backup_uid: None,
                    base_backup_id: None,
                    completed_at: None,
                };
                if job.action == DatabaseAction::Backup {
                    let backup = self
                        .get(
                            "Backup",
                            &format!("backup-{}", job.operation_id),
                            Some(&b.namespace),
                        )
                        .await?
                        .ok_or(ExecutionError::Ownership)?;
                    self.owned(&backup, job)?;
                    if backup.data["status"]["phase"] == "failed" {
                        return Err(ExecutionError::Runtime);
                    }
                    if backup.data["status"]["phase"] != "completed" {
                        return Ok(DatabaseStep::Pending);
                    }
                    proof.backup_uid = backup.metadata.uid;
                    proof.base_backup_id = backup.data["status"]["backupId"]
                        .as_str()
                        .map(str::to_owned);
                    proof.completed_at = backup.data["status"]["stoppedAt"]
                        .as_str()
                        .and_then(|s| s.parse().ok());
                    if proof.backup_uid.is_none()
                        || proof.base_backup_id.is_none()
                        || proof.completed_at.is_none()
                    {
                        return Err(ExecutionError::Runtime);
                    }
                }
                if job.action == DatabaseAction::ConfigureBackup
                    && !p.suspended
                    && let Some(latest) = self
                        .guarded(
                            claim,
                            authority,
                            latest_scheduled_backup(
                                &self.client,
                                &b.namespace,
                                &proof.scheduled_backup_uid,
                            ),
                        )
                        .await?
                {
                    proof.backup_uid = latest.metadata.uid.clone();
                    proof.base_backup_id = latest.data["status"]["backupId"]
                        .as_str()
                        .map(str::to_owned);
                    proof.completed_at = latest.data["status"]["stoppedAt"]
                        .as_str()
                        .and_then(|v| v.parse().ok());
                }
                let current = self
                    .get("Cluster", "postgres", Some(&b.namespace))
                    .await?
                    .ok_or(ExecutionError::Ownership)?;
                if current.metadata.uid.as_ref() != Some(&p.source_cnpg_uid) {
                    return Err(ExecutionError::Ownership);
                }
                self.guard(claim, authority).await?;
                Ok(DatabaseStep::Complete(DatabaseCompletionProof {
                    installation_id: b.id.clone(),
                    database_id: job.database.as_ref().map(|d| d.id.clone()),
                    cnpg_uid: p.source_cnpg_uid.clone(),
                    revision: job.revision,
                    state: DatabaseProofState::Ready,
                    endpoint_ready: false,
                    copy_sha256: None,
                    backup: Some(Box::new(proof)),
                }))
            }
            DatabaseExecutionPhase::Uncertain => Err(ExecutionError::Conflict),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn paginated_history_over_100_keeps_manual_and_failed_entries_out_of_scheduled_proof() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let backup = |name: &str, phase: &str, owner: &str, time: &str| json!({"apiVersion":"postgresql.cnpg.io/v1","kind":"Backup","metadata":{"name":name,"uid":format!("uid-{name}"),"ownerReferences":[{"apiVersion":"postgresql.cnpg.io/v1","kind":"ScheduledBackup","name":"daily","uid":owner}]},"spec":{"cluster":{"name":"postgres"}},"status":{"phase":phase,"stoppedAt":time,"backupId":"20261025T010203"}});
        let first = (0..100)
            .map(|n| {
                backup(
                    &format!("old-{n}"),
                    "completed",
                    "schedule-owned",
                    "2026-10-25T01:00:00Z",
                )
            })
            .collect::<Vec<_>>();
        let second = vec![
            backup(
                "scheduled-latest",
                "completed",
                "schedule-owned",
                "2026-10-25T02:00:00Z",
            ),
            backup(
                "manual-newer",
                "completed",
                "other-owner",
                "2026-10-25T03:00:00Z",
            ),
            backup(
                "failed-newest",
                "failed",
                "schedule-owned",
                "2026-10-25T04:00:00Z",
            ),
        ];
        let pages = [
            json!({"apiVersion":"postgresql.cnpg.io/v1","kind":"BackupList","metadata":{"continue":"next-page","resourceVersion":"200"},"items":first}),
            json!({"apiVersion":"postgresql.cnpg.io/v1","kind":"BackupList","metadata":{"continue":"","resourceVersion":"200"},"items":second}),
        ];
        let server = tokio::spawn(async move {
            for (index, page) in pages.into_iter().enumerate() {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut input = vec![];
                while !input.ends_with(b"\r\n\r\n") {
                    let byte = stream.read_u8().await.unwrap();
                    input.push(byte);
                    assert!(input.len() < 8192);
                }
                let request = String::from_utf8(input).unwrap();
                assert!(request.starts_with("GET "));
                if index == 1 {
                    assert!(request.contains("continue=next-page"));
                }
                let body = serde_json::to_vec(&page).unwrap();
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",body.len()).as_bytes()).await.unwrap();
                stream.write_all(&body).await.unwrap();
            }
        });
        let client = kube::Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap();
        let latest = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            latest_scheduled_backup(&client, "owned", "schedule-owned"),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(latest.metadata.name.as_deref(), Some("scheduled-latest"));
        assert!(
            serde_json::to_vec(&latest).unwrap().len() < 4096,
            "one bounded proof candidate, never the history payload"
        );
        server.await.unwrap();
    }
}
