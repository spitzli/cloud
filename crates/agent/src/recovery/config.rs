//! Operator-installed concrete recovery profiles. Keys/recipients/URLs are never API input.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spitzli_agent_protocol::recovery::RecoveryProfile;
use spitzli_object_storage::{Credentials, S3Config, Store};
use spitzli_supervisor::{ExecutionError, Result, RuntimeSpec};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryMode {
    #[default]
    BackupAndRestore,
    RestoreOnly,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryRuntimeConfig {
    #[serde(default)]
    pub mode: RecoveryMode,
    pub version: u8,
    pub installation: String,
    pub docker_socket: PathBuf,
    pub storage_socket: PathBuf,
    pub staging_directory: PathBuf,
    pub custody_directory: PathBuf,
    pub replay_directory: PathBuf,
    pub core_signers: PathBuf,
    pub operator_signers: PathBuf,
    pub profiles: Vec<RecoveryProfileConfig>,
    pub targets: Vec<RecoveryTarget>,
    #[serde(default)]
    pub kubernetes_source: Option<KubernetesSource>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KubernetesSource {
    pub kubeconfig: PathBuf,
    pub context: String,
    pub platform_id: String,
    pub helper_image: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryProfileConfig {
    pub id: String,
    pub profile: RecoveryProfile,
    #[serde(rename = "keyCustodyID")]
    pub key_custody_id: String,
    pub destination: S3Config,
    pub write_credentials: PathBuf,
    pub restore_credentials: PathBuf,
    pub age_recipient: String,
    pub age_identity: PathBuf,
    pub independent_files: BTreeMap<String, PathBuf>,
    pub source: RecoverySource,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum RecoverySource {
    Ntfy {
        runtime: Box<RuntimeSpec>,
        #[serde(rename = "runtimeID")]
        runtime_id: String,
    },
    Mail {
        core: Box<PostgresEndpoint>,
        metadata: PostgresEndpoint,
        #[serde(rename = "runtimeLabels")]
        runtime_labels: BTreeMap<String, String>,
        #[serde(rename = "ceLabels")]
        ce_labels: BTreeMap<String, String>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryTarget {
    pub id: String,
    pub profile: RecoveryProfile,
    pub capacity_bytes: u64,
    pub nonce: String,
    pub configuration: TargetConfiguration,
    pub tls_certificate: PathBuf,
    pub tls_key: PathBuf,
    pub gateway_image: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum TargetConfiguration {
    Ntfy {
        runtime: Box<RuntimeSpec>,
    },
    Mail {
        core: PostgresEndpoint,
        metadata: Box<PostgresEndpoint>,
        #[serde(rename = "ceImage")]
        ce_image: String,
        #[serde(default, rename = "volumePlan")]
        volume_plan: BTreeMap<String, crate::storage::recovery_pool::ApprovedRecoveryVolume>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PostgresEndpoint {
    pub host_address: String,
    pub server_name: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password_file: PathBuf,
    pub authority: PathBuf,
}
pub fn private(path: &Path, limit: u64) -> Result<Vec<u8>> {
    crate::config::read_private(path, limit).map_err(|_| ExecutionError::Ownership)
}
fn slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
pub fn fingerprint(value: &impl Serialize) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(value)
                .map_err(|_| ExecutionError::Invalid("recovery profile encoding"))?
        )
    ))
}
impl RecoveryRuntimeConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let value: Self = serde_json::from_slice(&private(path, 1024 * 1024)?)
            .map_err(|_| ExecutionError::Invalid("recovery configuration"))?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate_restore_only(&self) -> Result<()> {
        self.validate_mode(true)
    }
    pub fn validate(&self) -> Result<()> {
        self.validate_mode(self.mode == RecoveryMode::RestoreOnly)
    }
    fn validate_mode(&self, readonly: bool) -> Result<()> {
        if self.version != 1
            || !slug(&self.installation)
            || self.profiles.is_empty()
            || self.profiles.len() > 8
            || self.targets.len() > 16
            || !self.docker_socket.is_absolute()
            || [
                Path::new("/var/run/docker.sock"),
                Path::new("/run/docker.sock"),
            ]
            .contains(&self.docker_socket.as_path())
        {
            return Err(ExecutionError::Invalid(
                "explicit isolated recovery runtime required",
            ));
        }
        // This implementation is qualified for local operator fixtures only. A real
        // provider/custody profile is a separate production activation input.
        if !cfg!(feature = "local-recovery-fixture") {
            return Err(ExecutionError::Unsupported(
                "production recovery custody is not activated",
            ));
        }
        if let Some(source) = &self.kubernetes_source {
            if !cfg!(feature = "kubernetes")
                || source.context.is_empty()
                || !slug(&source.platform_id)
                || !spitzli_supervisor::image_digest(&source.helper_image)
            {
                return Err(ExecutionError::Invalid(
                    "qualified Kubernetes recovery source",
                ));
            }
            if !readonly {
                private(&source.kubeconfig, 1024 * 1024)?;
            }
        }
        for directory in [
            &self.staging_directory,
            &self.custody_directory,
            &self.replay_directory,
        ] {
            crate::config::private_directory(directory).map_err(|_| ExecutionError::Ownership)?;
        }
        for path in [&self.core_signers, &self.operator_signers] {
            private(path, 65536)?;
        }
        let mut ids = std::collections::BTreeSet::new();
        for profile in &self.profiles {
            if !slug(&profile.id)
                || !ids.insert(&profile.id)
                || !slug(&profile.key_custody_id)
                || profile.independent_files.len() > 16
                || profile.age_recipient.len() > 128
            {
                return Err(ExecutionError::Invalid("recovery profile"));
            }
            let endpoint = url::Url::parse(&profile.destination.endpoint)
                .map_err(|_| ExecutionError::Invalid("recovery endpoint"))?;
            if endpoint
                .host_str()
                .and_then(|h| h.parse::<std::net::IpAddr>().ok())
                .is_none_or(|ip| !ip.is_loopback())
            {
                return Err(ExecutionError::Unsupported(
                    "only owned local recovery provider is qualified",
                ));
            }
            if !profile.write_credentials.is_absolute()
                || !profile.restore_credentials.is_absolute()
                || profile.write_credentials == profile.restore_credentials
            {
                return Err(ExecutionError::Invalid(
                    "separate recovery credential paths required",
                ));
            }
            let reader: serde_json::Value =
                serde_json::from_slice(&private(&profile.restore_credentials, 16384)?)
                    .map_err(|_| ExecutionError::Invalid("restore credentials"))?;
            let _: Credentials = serde_json::from_value(reader.clone())
                .map_err(|_| ExecutionError::Invalid("restore credentials"))?;
            if !readonly {
                let writer: serde_json::Value =
                    serde_json::from_slice(&private(&profile.write_credentials, 16384)?)
                        .map_err(|_| ExecutionError::Invalid("backup credentials"))?;
                let _: Credentials = serde_json::from_value(writer.clone())
                    .map_err(|_| ExecutionError::Invalid("backup credentials"))?;
                if writer["accessKeyId"] == reader["accessKeyId"] {
                    return Err(ExecutionError::Invalid(
                        "separate restore credentials required",
                    ));
                }
            }
            private(&profile.age_identity, 16384)?;
            for (name, path) in &profile.independent_files {
                if !slug(name) || !path.is_absolute() {
                    return Err(ExecutionError::Invalid("recovery material name"));
                }
            }
            match &profile.source {
                RecoverySource::Ntfy {
                    runtime,
                    runtime_id,
                } => {
                    runtime.validate()?;
                    if profile.profile != RecoveryProfile::NtfySqliteOfflineV1
                        || runtime_id.len() != 64
                        || runtime.arguments != ["serve"]
                        || runtime.volume.is_none()
                        || !runtime.mounts.is_empty()
                        || runtime
                            .hosted_access
                            .as_ref()
                            .is_some_and(|a| a.service_bindings.is_some())
                    {
                        return Err(ExecutionError::Invalid("ntfy consistency profile"));
                    }
                }
                RecoverySource::Mail {
                    core,
                    metadata,
                    runtime_labels,
                    ce_labels,
                } => {
                    core.validate()?;
                    metadata.validate()?;
                    if profile.profile != RecoveryProfile::MailInstallationV1
                        || runtime_labels.is_empty()
                        || ce_labels.is_empty()
                    {
                        return Err(ExecutionError::Invalid("Mail installation profile"));
                    }
                }
            }
        }
        let mut targets = std::collections::BTreeSet::new();
        for target in &self.targets {
            if !slug(&target.id)
                || !targets.insert(&target.id)
                || !(67108864..=64 * 1024 * 1024 * 1024).contains(&target.capacity_bytes)
                || !uuid::Uuid::parse_str(&target.nonce)
                    .is_ok_and(|n| n.to_string() == target.nonce && !n.is_nil())
                || !spitzli_supervisor::image_digest(&target.gateway_image)
            {
                return Err(ExecutionError::Invalid("fresh recovery target"));
            }
            private(&target.tls_certificate, 65536)?;
            private(&target.tls_key, 65536)?;
            match &target.configuration {
                TargetConfiguration::Ntfy { runtime } => runtime.validate()?,
                TargetConfiguration::Mail {
                    core,
                    metadata,
                    ce_image,
                    ..
                } => {
                    core.validate()?;
                    metadata.validate()?;
                    if !spitzli_supervisor::image_digest(ce_image) {
                        return Err(ExecutionError::Invalid("CE recovery image"));
                    }
                }
            }
        }
        Ok(())
    }
    pub fn public_capabilities(&self) -> Result<serde_json::Value> {
        let profiles=self.profiles.iter().filter(|_|self.mode != RecoveryMode::RestoreOnly).map(|p|Ok(serde_json::json!({"id":p.id,"profile":p.profile,"sha256":fingerprint(p)?,"keyCustodyID":p.key_custody_id,"productionQualified":false}))).collect::<Result<Vec<_>>>()?;
        let targets=self.targets.iter().map(|target|Ok(serde_json::json!({"id":target.id,"profile":target.profile,"sha256":fingerprint(target)?,"nonce":target.nonce,"capacityBytes":target.capacity_bytes,"runtime":match &target.configuration {TargetConfiguration::Ntfy{runtime}=>serde_json::to_value(runtime).map_err(|_|ExecutionError::Runtime)?,TargetConfiguration::Mail{..}=>serde_json::Value::Null}}))).collect::<Result<Vec<_>>>()?;
        let mut result = serde_json::json!({"version":1,"profiles":profiles,"targets":targets});
        for (value, target) in result["targets"]
            .as_array_mut()
            .ok_or(ExecutionError::Ownership)?
            .iter_mut()
            .zip(&self.targets)
        {
            value["volumePlan"] = match &target.configuration {
                TargetConfiguration::Mail { volume_plan, .. } => {
                    serde_json::to_value(volume_plan).map_err(|_| ExecutionError::Ownership)?
                }
                TargetConfiguration::Ntfy { .. } => serde_json::json!({}),
            };
        }
        Ok(result)
    }
    pub async fn store(&self, profile: &RecoveryProfileConfig, readonly: bool) -> Result<Store> {
        if !readonly && self.mode == RecoveryMode::RestoreOnly {
            return Err(ExecutionError::Denied);
        }
        let credentials: Credentials = serde_json::from_slice(&private(
            if readonly {
                &profile.restore_credentials
            } else {
                &profile.write_credentials
            },
            16384,
        )?)
        .map_err(|_| ExecutionError::Invalid("recovery credentials"))?;
        #[cfg(feature = "local-recovery-fixture")]
        {
            Store::loopback_test(profile.destination.clone(), credentials)
                .map_err(|_| ExecutionError::Runtime)
        }
        #[cfg(not(feature = "local-recovery-fixture"))]
        {
            let _ = (profile, credentials);
            Err(ExecutionError::Unsupported("unqualified recovery provider"))
        }
    }
}
impl PostgresEndpoint {
    pub fn validate(&self) -> Result<()> {
        if self.host_address.parse::<std::net::IpAddr>().is_err()
            || self.server_name.is_empty()
            || self.server_name.len() > 253
            || self.port == 0
            || !slug(&self.database)
            || !slug(&self.username)
        {
            return Err(ExecutionError::Invalid(
                "private recovery PostgreSQL endpoint",
            ));
        }
        if !self.password_file.is_absolute() || !self.authority.is_absolute() {
            return Err(ExecutionError::Invalid("private PostgreSQL custody paths"));
        }
        Ok(())
    }
}

/// Selects the existing declared target; the full immutable binding is checked here.
pub fn authorize_target(
    target: &RecoveryTarget,
    declared: &serde_json::Value,
    pool: &crate::storage::recovery_pool::RecoveryPoolIdentity,
) -> Result<()> {
    if declared["profileID"] != target.id
        || declared["profile"]
            != serde_json::to_value(target.profile).map_err(|_| ExecutionError::Ownership)?
        || declared["nonce"] != target.nonce
        || declared["sha256"] != fingerprint(&(target, pool))?
        || declared["storageIdentity"]
            != serde_json::to_value(pool).map_err(|_| ExecutionError::Ownership)?
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}
#[cfg(test)]
mod binding_tests {
    use super::*;
    #[test]
    fn target_indirection_preserves_inline_bytes_and_signed_fingerprints() {
        #[derive(Serialize, Deserialize)]
        struct InlineNtfy {
            kind: String,
            runtime: RuntimeSpec,
        }
        #[derive(Serialize, Deserialize)]
        struct InlineMail {
            kind: String,
            core: PostgresEndpoint,
            metadata: PostgresEndpoint,
            #[serde(rename = "ceImage")]
            ce_image: String,
            #[serde(default, rename = "volumePlan")]
            volume_plan: BTreeMap<String, crate::storage::recovery_pool::ApprovedRecoveryVolume>,
        }
        let runtime = serde_json::json!({"version":1,"installation":"wire-test","projectId":"123","resourceId":"124","revision":1,"image":format!("registry.invalid/ntfy@sha256:{}","a".repeat(64)),"arguments":["serve"],"cpuMillicores":100,"memoryBytes":67108864,"ephemeralBytes":1048576,"replicas":1,"port":8080,"healthPath":"/health","volume":{"mountPath":"/data","sizeBytes":67108864}});
        let endpoint = serde_json::json!({"hostAddress":"127.0.0.1","serverName":"localhost","port":5432,"database":"owned","username":"recovery","passwordFile":"/owned/secret-reference","authority":"/owned/ca-reference"});
        for value in [
            serde_json::json!({"kind":"ntfy","runtime":runtime}),
            serde_json::json!({"kind":"mail","core":endpoint,"metadata":endpoint,"ceImage":format!("registry.invalid/mail@sha256:{}","b".repeat(64))}),
        ] {
            let current: TargetConfiguration = serde_json::from_value(value.clone()).unwrap();
            let before = if value["kind"] == "ntfy" {
                serde_json::to_vec(&serde_json::from_value::<InlineNtfy>(value).unwrap()).unwrap()
            } else {
                serde_json::to_vec(&serde_json::from_value::<InlineMail>(value).unwrap()).unwrap()
            };
            assert_eq!(serde_json::to_vec(&current).unwrap(), before);
            assert_eq!(
                fingerprint(&current).unwrap(),
                format!("{:x}", Sha256::digest(before))
            );
        }
    }
    #[test]
    fn source_indirection_preserves_inline_profile_bytes_and_signed_fingerprints() {
        #[derive(Serialize, Deserialize)]
        struct InlineNtfy {
            kind: String,
            runtime: RuntimeSpec,
            #[serde(rename = "runtimeID")]
            runtime_id: String,
        }
        #[derive(Serialize, Deserialize)]
        struct InlineMail {
            kind: String,
            core: PostgresEndpoint,
            metadata: PostgresEndpoint,
            #[serde(rename = "runtimeLabels")]
            runtime_labels: BTreeMap<String, String>,
            #[serde(rename = "ceLabels")]
            ce_labels: BTreeMap<String, String>,
        }
        let runtime = serde_json::json!({"version":1,"installation":"wire-test","projectId":"123","resourceId":"124","revision":1,"image":format!("registry.invalid/ntfy@sha256:{}","a".repeat(64)),"arguments":["serve"],"cpuMillicores":100,"memoryBytes":67108864,"ephemeralBytes":1048576,"replicas":1,"port":8080,"healthPath":"/health","volume":{"mountPath":"/data","sizeBytes":67108864}});
        let endpoint = serde_json::json!({"hostAddress":"127.0.0.1","serverName":"localhost","port":5432,"database":"owned","username":"recovery","passwordFile":"/owned/secret-reference","authority":"/owned/ca-reference"});
        for value in [
            serde_json::json!({"kind":"ntfy","runtime":runtime,"runtimeID":"a".repeat(64)}),
            serde_json::json!({"kind":"mail","core":endpoint,"metadata":endpoint,"runtimeLabels":{"owned":"runtime"},"ceLabels":{"owned":"mail"}}),
        ] {
            let current: RecoverySource = serde_json::from_value(value.clone()).unwrap();
            let before = if value["kind"] == "ntfy" {
                serde_json::to_vec(&serde_json::from_value::<InlineNtfy>(value).unwrap()).unwrap()
            } else {
                serde_json::to_vec(&serde_json::from_value::<InlineMail>(value).unwrap()).unwrap()
            };
            assert_eq!(serde_json::to_vec(&current).unwrap(), before);
            assert_eq!(
                fingerprint(&current).unwrap(),
                format!("{:x}", Sha256::digest(before))
            );
        }
    }
    fn target() -> RecoveryTarget {
        serde_json::from_value(serde_json::json!({"id":"fresh","profile":"mail-installation-v1","capacityBytes":67108864,"nonce":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","configuration":{"kind":"mail","core":{"hostAddress":"127.0.0.1","serverName":"localhost","port":5432,"database":"core","username":"recovery","passwordFile":"/independent/pw","authority":"/independent/ca"},"metadata":{"hostAddress":"127.0.0.1","serverName":"localhost","port":5433,"database":"private","username":"recovery","passwordFile":"/independent/pw","authority":"/independent/ca"},"ceImage":format!("example.test/ce@sha256:{}","a".repeat(64))},"tlsCertificate":"/independent/cert","tlsKey":"/independent/key","gatewayImage":format!("example.test/gateway@sha256:{}","b".repeat(64))})).unwrap()
    }
    #[test]
    fn old_same_id_claim_cannot_authorize_a_changed_target_nonce_config_or_pool() {
        let mut target = target();
        let configuration:crate::storage::StorageConfig=serde_json::from_value(serde_json::json!({"root":"/owned/pool","node":"owned","clusterID":"123","poolID":"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb","capacityBytes":1073741824,"reserveBytes":67108864})).unwrap();
        let mut pool = crate::storage::recovery_pool::RecoveryPoolIdentity {
            configuration,
            pool_uid: "owned-device-inode".into(),
            revision: "a".repeat(64),
        };
        let declared = serde_json::json!({"profileID":target.id,"profile":target.profile,"sha256":fingerprint(&(&target,&pool)).unwrap(),"nonce":target.nonce,"storageIdentity":pool});
        authorize_target(&target, &declared, &pool).unwrap();
        target.nonce = "cccccccc-cccc-4ccc-8ccc-cccccccccccc".into();
        assert!(
            authorize_target(&target, &declared, &pool).is_err(),
            "same profile ID cannot authorize replacement target"
        );
        target = self::target();
        pool.configuration.pool_id = "dddddddd-dddd-4ddd-8ddd-dddddddddddd".into();
        assert!(authorize_target(&target, &declared, &pool).is_err());
    }
}

#[cfg(all(test, feature = "local-recovery-fixture"))]
mod readonly_tests {
    use super::*;
    #[test]
    fn source_unavailable_restore_profile_does_not_require_any_backup_writer_credential() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        for name in ["reader", "identity", "signers"] {
            std::fs::write(root.path().join(name), if name=="reader" { br#"{"accessKeyId":"restore-only-unit","secretAccessKey":"private-unit-secret"}"#.as_slice() } else { b"private-unit-custody".as_slice() }).unwrap();
            std::fs::set_permissions(
                root.path().join(name),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        let endpoint = serde_json::json!({"hostAddress":"127.0.0.1","serverName":"source-unavailable.example.test","port":5432,"database":"missing_source","username":"missing_source","passwordFile":root.path().join("lost-source-password"),"authority":root.path().join("lost-source-ca")});
        let config:RecoveryRuntimeConfig=serde_json::from_value(serde_json::json!({"version":1,"installation":"unit","dockerSocket":root.path().join("owned-engine.sock"),"storageSocket":root.path().join("storage.sock"),"stagingDirectory":root.path(),"custodyDirectory":root.path(),"replayDirectory":root.path(),"coreSigners":root.path().join("signers"),"operatorSigners":root.path().join("signers"),"profiles":[{"id":"mail-unit","profile":"mail-installation-v1","keyCustodyID":"operator-unit","destination":{"id":"unit","endpoint":"http://127.0.0.1:9000","region":"local","bucket":"owned-unit","namespace":"owned"},"writeCredentials":root.path().join("missing-writer"),"restoreCredentials":root.path().join("reader"),"ageRecipient":"age1unit","ageIdentity":root.path().join("identity"),"independentFiles":{},"source":{"kind":"mail","core":endpoint,"metadata":endpoint,"runtimeLabels":{"owned":"unit"},"ceLabels":{"owned":"unit"}}}],"targets":[]})).unwrap();
        assert!(!config.profiles[0].write_credentials.exists());
        assert!(
            config.validate_restore_only().is_ok(),
            "readonly cold recovery must not load absent backup credentials or source files"
        );
        let mut readonly_config = serde_json::to_value(&config).unwrap();
        readonly_config["mode"] = serde_json::json!("restore-only");
        let file = root.path().join("restore-config.json");
        std::fs::write(&file, serde_json::to_vec(&readonly_config).unwrap()).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(RecoveryRuntimeConfig::load(&file).is_ok());
        assert!(
            config.validate().is_err(),
            "backup still requires its independent writer credential"
        );
    }
}
