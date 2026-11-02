//! Operator-owned, pinned signed releases. Browser commands select a reviewed entry only.
use crate::{invalid, unavailable};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};
use webdock_plugin_protocol::{
    Manifest, Result,
    release::{ReleaseInput, sha256, strict_json, verify_release},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogFile {
    version: u8,
    #[serde(rename = "platformID")]
    platform_id: String,
    keys: Vec<Key>,
    targets: Vec<Target>,
    releases: Vec<Release>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Key {
    #[serde(rename = "registryURL")]
    registry_url: String,
    kid: String,
    key: Jwk,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Jwk {
    kty: String,
    crv: String,
    x: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub label: String,
    #[serde(rename = "clusterID")]
    pub cluster_id: String,
    pub policy: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Release {
    id: String,
    version: String,
    #[serde(rename = "registryURL")]
    registry_url: String,
    kid: String,
    jws: String,
    manifest_raw: String,
    descriptor: String,
    recipe: Value,
    supported_features: Vec<String>,
    #[serde(rename = "targetIDs")]
    target_ids: Vec<String>,
}
pub struct Catalog {
    pub(crate) platform_id: String,
    pub(crate) entries: BTreeMap<(String, String), CatalogEntry>,
}
pub(crate) struct CatalogEntry {
    pub manifest: Manifest,
    pub manifest_sha256: String,
    pub signer_sha256: String,
    pub registry_url: String,
    pub kid: String,
    pub jws: String,
    pub recipe: Value,
    pub supported_features: Vec<String>,
    pub targets: BTreeMap<String, (Target, String)>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogDescriptor {
    #[serde(rename = "catalogID")]
    pub catalog_id: String,
    #[serde(rename = "pluginID")]
    pub plugin_id: String,
    pub version: String,
    pub manifest_sha256: String,
    pub image: String,
    #[serde(rename = "registryURL")]
    pub registry_url: String,
    #[serde(rename = "signerKeyID")]
    pub signer_key_id: String,
    pub capabilities: Vec<String>,
    pub supported_features: Vec<String>,
    pub targets: Vec<CatalogTargetDescriptor>,
    pub configuration_schema: Value,
    pub schema_version: u64,
    pub readable_schema: Value,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogTargetDescriptor {
    pub id: String,
    pub label: String,
    pub review_sha256: String,
}

impl Catalog {
    pub fn load(path: &Path) -> Result<Self> {
        use std::{
            io::Read,
            os::unix::fs::{MetadataExt, OpenOptionsExt},
        };
        if !path.is_absolute() {
            return Err(invalid("Absolute catalog path required"));
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(path)
            .map_err(|_| unavailable())?;
        let meta = file.metadata().map_err(|_| unavailable())?;
        if !meta.is_file()
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.mode() & 0o077 != 0
            || meta.nlink() != 1
            || meta.len() > 8 * 1024 * 1024
        {
            return Err(invalid("Untrusted catalog file"));
        }
        let mut bytes = Vec::new();
        file.take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| unavailable())?;
        Self::from_bytes(&bytes)
    }
    pub fn from_bytes(raw: &[u8]) -> Result<Self> {
        if raw.len() > 8 * 1024 * 1024 {
            return Err(invalid("Catalog exceeds bounds"));
        }
        let file: CatalogFile =
            serde_json::from_value(strict_json(raw)?).map_err(|_| invalid("Invalid catalog"))?;
        if file.version != 1
            || !crate::slug(&file.platform_id)
            || file.platform_id.len() > 24
            || file.keys.len() > 32
            || file.targets.len() > 32
            || file.releases.len() > 32
        {
            return Err(invalid("Invalid catalog bounds"));
        }
        let mut keys = BTreeMap::new();
        for key in file.keys {
            let url =
                url::Url::parse(&key.registry_url).map_err(|_| invalid("Invalid registry"))?;
            if url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || key.kid.is_empty()
                || key.kid.len() > 128
                || key.key.kty != "OKP"
                || key.key.crv != "Ed25519"
            {
                return Err(invalid("Invalid trusted key"));
            }
            let bytes: [u8; 32] = URL_SAFE_NO_PAD
                .decode(key.key.x)
                .map_err(|_| invalid("Invalid trusted key"))?
                .try_into()
                .map_err(|_| invalid("Invalid trusted key"))?;
            let public = ed25519_dalek::VerifyingKey::from_bytes(&bytes)
                .map_err(|_| invalid("Invalid trusted key"))?;
            if keys.insert((key.registry_url, key.kid), public).is_some() {
                return Err(invalid("Duplicate signing key"));
            }
        }
        let mut targets = BTreeMap::new();
        for target in file.targets {
            if !crate::slug(&target.id)
                || target.label.is_empty()
                || target.label.len() > 160
                || !crate::data::revision(&target.cluster_id)
                || !target.policy.is_object()
                || serde_json::to_vec(&target.policy)
                    .map_err(|_| unavailable())?
                    .len()
                    > 24000
                || targets.insert(target.id.clone(), target).is_some()
            {
                return Err(invalid("Invalid catalog target"));
            }
        }
        let mut entries = BTreeMap::new();
        for release in file.releases {
            let key = keys
                .get(&(release.registry_url.clone(), release.kid.clone()))
                .ok_or_else(|| invalid("Unknown signing key"))?;
            let verified = verify_release(&ReleaseInput {
                registry_url: &release.registry_url,
                id: &release.id,
                version: &release.version,
                kid: &release.kid,
                key,
                jws: &release.jws,
                manifest_raw: &release.manifest_raw,
                descriptor: &release.descriptor,
            })?;
            verified.manifest.require_action_only_lifecycle()?;
            verified
                .manifest
                .require_host_services(if verified.manifest.id() == "mail" {
                    &[
                        ("data", 1),
                        ("http", 1),
                        ("jobs", 1),
                        ("storage", 1),
                        ("native-mail-control", 1),
                    ]
                } else {
                    &[("data", 1), ("http", 1), ("jobs", 1), ("storage", 1)]
                })?;
            if !release.recipe.is_object()
                || serde_json::to_vec(&release.recipe)
                    .map_err(|_| unavailable())?
                    .len()
                    > 24000
                || release.supported_features.len() > 32
                || release.target_ids.is_empty()
                || release.target_ids.len() > 32
            {
                return Err(invalid("Invalid installation recipe"));
            }
            let mut features = std::collections::BTreeSet::new();
            for id in &release.supported_features {
                if !features.insert(id)
                    || !verified.manifest.features().iter().any(|f| f["id"] == *id)
                {
                    return Err(invalid("Unknown or duplicate supported feature"));
                }
            }
            let mut selected = BTreeMap::new();
            for id in &release.target_ids {
                let target = targets
                    .get(id)
                    .ok_or_else(|| invalid("Unknown catalog target"))?;
                let review=sha256(serde_json::to_vec(&serde_json::json!({"v":1,"platformID":file.platform_id,"registryURL":release.registry_url,"kid":release.kid,"signerSha256":verified.signer_sha256,"manifestSha256":verified.manifest_sha256,"recipe":release.recipe,"supportedFeatures":release.supported_features,"target":target})).map_err(|_|unavailable())?);
                if selected
                    .insert(id.clone(), (target.clone(), review))
                    .is_some()
                {
                    return Err(invalid("Duplicate catalog target"));
                }
            }
            let entry = CatalogEntry {
                manifest: verified.manifest,
                manifest_sha256: verified.manifest_sha256,
                signer_sha256: verified.signer_sha256,
                registry_url: release.registry_url,
                kid: release.kid,
                jws: release.jws,
                recipe: release.recipe,
                supported_features: release.supported_features,
                targets: selected,
            };
            if entries
                .insert((release.id, release.version), entry)
                .is_some()
            {
                return Err(invalid("Duplicate catalog release"));
            }
        }
        Ok(Self {
            platform_id: file.platform_id,
            entries,
        })
    }
    pub fn descriptors(&self) -> Vec<CatalogDescriptor> {
        self.entries
            .iter()
            .map(|((id, version), e)| CatalogDescriptor {
                catalog_id: id.clone(),
                plugin_id: id.clone(),
                version: version.clone(),
                manifest_sha256: e.manifest_sha256.clone(),
                image: e.manifest.as_json()["image"].as_str().unwrap().into(),
                registry_url: e.registry_url.clone(),
                signer_key_id: e.kid.clone(),
                capabilities: e.manifest.as_json()["capabilities"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect(),
                supported_features: e.supported_features.clone(),
                targets: e
                    .targets
                    .values()
                    .map(|(t, review)| CatalogTargetDescriptor {
                        id: t.id.clone(),
                        label: t.label.clone(),
                        review_sha256: review.clone(),
                    })
                    .collect(),
                configuration_schema: e.manifest.as_json()["configuration"].clone(),
                schema_version: e.manifest.as_json()["schemaVersion"].as_u64().unwrap(),
                readable_schema: e.manifest.as_json()["readableSchema"].clone(),
            })
            .collect()
    }
}
