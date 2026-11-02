//! Native mailbox configuration. No SMTP routing, relay material or arbitrary engine commands.
use crate::mail::{MailAction, MailLease, MailMaterial, MailOutcome, MailResources, MailSpec};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, Copy)]
pub struct InvalidConfiguration;
type Result<T> = std::result::Result<T, InvalidConfiguration>;
fn id(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('0')
        && value.len() <= 19
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<i64>().is_ok()
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn provider(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn runtime(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn domain(value: &str) -> bool {
    value.len() <= 253
        && value.contains('.')
        && value.split('.').all(|p| {
            !p.is_empty()
                && p.len() <= 63
                && !p.starts_with('-')
                && !p.ends_with('-')
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}
fn digest(value: &impl Serialize) -> Result<String> {
    crate::mail::fingerprint(value).map_err(|_| InvalidConfiguration)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigurationMode {
    Apply,
    Restrict,
}
impl ConfigurationMode {
    pub fn advances_configuration(self) -> bool {
        self == Self::Apply
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationTarget {
    pub instance_key: String,
    pub hostname: String,
    pub image: String,
    pub resources: MailResources,
    pub management_sha256: String,
    pub certificate_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationDomain {
    pub id: String,
    pub name: String,
    #[serde(rename = "providerID")]
    pub provider_id: Option<String>,
    pub ownership_verified: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationMailbox {
    pub id: String,
    #[serde(rename = "domainID")]
    pub domain_id: String,
    pub local_part: String,
    pub quota_bytes: u64,
    #[serde(rename = "providerID")]
    pub provider_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationIdentity {
    pub id: String,
    pub subject: String,
    #[serde(rename = "providerID")]
    pub provider_id: Option<String>,
    pub enabled: bool,
    #[serde(rename = "mailboxIDs")]
    pub mailbox_ids: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationGrant {
    #[serde(rename = "mailboxID")]
    pub mailbox_id: String,
    pub subject: String,
    pub revision: String,
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationSnapshot {
    pub domains: Vec<ConfigurationDomain>,
    pub mailboxes: Vec<ConfigurationMailbox>,
    pub identities: Vec<ConfigurationIdentity>,
    /// Apply carries the complete effective enabled set; Restrict carries only exact revoked rows.
    pub grants: Vec<ConfigurationGrant>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationSpec {
    pub version: u8,
    #[serde(rename = "operationID")]
    pub operation_id: String,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "clusterID")]
    pub cluster_id: String,
    pub revision: String,
    pub service_revision: String,
    pub agent_generation: i32,
    pub mode: ConfigurationMode,
    pub target: ConfigurationTarget,
    pub material_sha256: String,
    pub snapshot: ConfigurationSnapshot,
}
impl ConfigurationSpec {
    pub fn validate(&self) -> Result<()> {
        let service_revision = self
            .service_revision
            .parse::<i64>()
            .map_err(|_| InvalidConfiguration)?;
        let target = MailSpec {
            version: self.version,
            operation_id: self.operation_id.clone(),
            customer_id: self.customer_id.clone(),
            cluster_id: self.cluster_id.clone(),
            revision: service_revision,
            agent_generation: self.agent_generation,
            instance_key: self.target.instance_key.clone(),
            hostname: self.target.hostname.clone(),
            image: self.target.image.clone(),
            resources: self.target.resources,
            material_sha256: self.target.management_sha256.clone(),
            action: MailAction::Ensure,
        };
        target.validate().map_err(|_| InvalidConfiguration)?;
        if !id(&self.revision)
            || !id(&self.service_revision)
            || !hash(&self.material_sha256)
            || !hash(&self.target.certificate_sha256)
        {
            return Err(InvalidConfiguration);
        }
        self.snapshot.validate(self.mode)?;
        if serde_json::to_vec(self)
            .map_err(|_| InvalidConfiguration)?
            .len()
            > 262144
        {
            return Err(InvalidConfiguration);
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> Result<String> {
        self.validate()?;
        digest(self)
    }
    pub fn contains(&self, resource: &ConfigurationResource) -> bool {
        if self.mode == ConfigurationMode::Restrict
            && resource.kind != ConfigurationResourceKind::Identity
        {
            return false;
        }
        match resource.kind {
            ConfigurationResourceKind::Domain => {
                self.snapshot.domains.iter().any(|v| v.id == resource.id)
            }
            ConfigurationResourceKind::Mailbox => {
                self.snapshot.mailboxes.iter().any(|v| v.id == resource.id)
            }
            ConfigurationResourceKind::Identity => {
                self.snapshot.identities.iter().any(|v| v.id == resource.id)
            }
        }
    }
}
impl ConfigurationSnapshot {
    fn validate(&self, mode: ConfigurationMode) -> Result<()> {
        if self.domains.len() > 32
            || self.mailboxes.len() > 64
            || self.identities.len() > 128
            || self.grants.len() > 256
        {
            return Err(InvalidConfiguration);
        }
        let mut domains = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut provider_ids = BTreeSet::new();
        for d in &self.domains {
            if !id(&d.id)
                || !domain(&d.name)
                || !domains.insert(&d.id)
                || !names.insert(&d.name)
                || !d.ownership_verified
                || d.provider_id
                    .as_ref()
                    .is_some_and(|p| !provider(p) || !provider_ids.insert(p))
            {
                return Err(InvalidConfiguration);
            }
        }
        let mut mailboxes = BTreeSet::new();
        let mut addresses = BTreeSet::new();
        let mut bytes = 0u64;
        provider_ids.clear();
        for m in &self.mailboxes {
            bytes = bytes
                .checked_add(m.quota_bytes)
                .ok_or(InvalidConfiguration)?;
            if !id(&m.id)
                || (mode == ConfigurationMode::Apply && !domains.contains(&m.domain_id))
                || !id(&m.domain_id)
                || !mailboxes.insert(&m.id)
                || !addresses.insert((&m.domain_id, &m.local_part))
                || m.local_part.is_empty()
                || m.local_part.len() > 64
                || !m.local_part.split(['.', '_', '+', '-']).all(|p| {
                    !p.is_empty()
                        && p.bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                })
                || !(1048576..=8589934592).contains(&m.quota_bytes)
                || m.provider_id
                    .as_ref()
                    .is_some_and(|p| !provider(p) || !provider_ids.insert(p))
            {
                return Err(InvalidConfiguration);
            }
        }
        if mode == ConfigurationMode::Apply && bytes > 8589934592 {
            return Err(InvalidConfiguration);
        }
        let mut identities = BTreeSet::new();
        let mut subjects = BTreeSet::new();
        let mut memberships = BTreeSet::new();
        for i in &self.identities {
            let groups = i.mailbox_ids.iter().collect::<BTreeSet<_>>();
            if !id(&i.id)
                || !id(&i.subject)
                || !identities.insert(&i.id)
                || !subjects.insert(&i.subject)
                || i.enabled != !groups.is_empty()
                || groups.len() != i.mailbox_ids.len()
                || !groups.is_subset(&mailboxes)
                || i.provider_id
                    .as_ref()
                    .is_some_and(|p| !provider(p) || !provider_ids.insert(p))
            {
                return Err(InvalidConfiguration);
            }
            for mailbox in groups {
                memberships.insert((mailbox, &i.subject));
            }
        }
        let mut grants = BTreeSet::new();
        let mut enabled = BTreeSet::new();
        for g in &self.grants {
            if !id(&g.mailbox_id)
                || !id(&g.subject)
                || !id(&g.revision)
                || !grants.insert((&g.mailbox_id, &g.subject))
            {
                return Err(InvalidConfiguration);
            }
            if g.enabled {
                enabled.insert((&g.mailbox_id, &g.subject));
            }
        }
        if mode == ConfigurationMode::Apply && enabled != memberships {
            return Err(InvalidConfiguration);
        }
        match mode {
            ConfigurationMode::Apply if self.grants.iter().any(|g| !g.enabled) => {
                return Err(InvalidConfiguration);
            }
            ConfigurationMode::Restrict
                if !self.domains.is_empty()
                    || self.mailboxes.iter().any(|m| m.provider_id.is_none())
                    || self.identities.iter().any(|i| i.provider_id.is_none())
                    || self.grants.iter().any(|g| g.enabled) =>
            {
                return Err(InvalidConfiguration);
            }
            ConfigurationMode::Apply | ConfigurationMode::Restrict => {}
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationCredential {
    pub username: String,
    pub password: String,
}
impl ConfigurationCredential {
    fn validate(&self) -> bool {
        !self.username.is_empty()
            && self.username.len() <= 320
            && !self.username.chars().any(|c| c.is_control() || c == ':')
            && (16..=128).contains(&self.password.len())
            && self
                .password
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_!@.-".contains(&b))
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationMaterial {
    pub administrator: ConfigurationCredential,
    pub identities: BTreeMap<String, ConfigurationCredential>,
}
impl ConfigurationMaterial {
    pub fn fingerprint(&self) -> Result<String> {
        digest(self)
    }
    pub fn management_fingerprint(&self) -> Result<String> {
        MailMaterial::Credentials {
            bootstrap_password: None,
            username: self.administrator.username.clone(),
            password: self.administrator.password.clone(),
        }
        .fingerprint()
        .map_err(|_| InvalidConfiguration)
    }
}
#[derive(Clone)]
pub struct ConfigurationClaim {
    pub spec: ConfigurationSpec,
    pub lease: MailLease,
    pub lease_until: DateTime<Utc>,
    pub material: ConfigurationMaterial,
    pub checkpoints: Vec<ConfigurationCheckpoint>,
}
impl ConfigurationClaim {
    pub fn validate(&self, now: DateTime<Utc>) -> Result<()> {
        self.spec.validate()?;
        self.lease.validate().map_err(|_| InvalidConfiguration)?;
        if self.lease_until <= now
            || self.lease_until.signed_duration_since(now).num_seconds() > 120
            || self.material.fingerprint()? != self.spec.material_sha256
            || self.material.management_fingerprint()? != self.spec.target.management_sha256
            || !self.material.administrator.validate()
            || self.material.identities.len()
                != self
                    .spec
                    .snapshot
                    .identities
                    .iter()
                    .filter(|i| {
                        self.spec.mode == ConfigurationMode::Apply
                            && (i.enabled || i.provider_id.is_some())
                    })
                    .count()
            || self.checkpoints.len() > 449
        {
            return Err(InvalidConfiguration);
        }
        for identity in self.spec.snapshot.identities.iter().filter(|i| {
            self.spec.mode == ConfigurationMode::Apply && (i.enabled || i.provider_id.is_some())
        }) {
            let c = self
                .material
                .identities
                .get(&identity.id)
                .ok_or(InvalidConfiguration)?;
            if !c.validate()
                || c.username != format!("wd-{}@{}", identity.id, self.spec.target.hostname)
            {
                return Err(InvalidConfiguration);
            }
        }
        self.validate_history()
    }
    pub fn validate_history(&self) -> Result<()> {
        if self.checkpoints.len() > 449 {
            return Err(InvalidConfiguration);
        }
        let mut runtime_id = None;
        let mut handed = BTreeMap::new();
        let mut applied = BTreeSet::new();
        for checkpoint in &self.checkpoints {
            checkpoint.validate()?;
            if checkpoint.lease.lease_token != self.lease.lease_token {
                return Err(InvalidConfiguration);
            }
            match &checkpoint.phase {
                ConfigurationCheckpointPhase::Bound {} if runtime_id.is_none() => {
                    runtime_id = Some(&checkpoint.runtime_id)
                }
                ConfigurationCheckpointPhase::Bound {} => return Err(InvalidConfiguration),
                ConfigurationCheckpointPhase::Handoff {
                    resource,
                    request_sha256,
                } => {
                    if runtime_id != Some(&checkpoint.runtime_id)
                        || !self.spec.contains(resource)
                        || handed.insert(resource, request_sha256).is_some()
                    {
                        return Err(InvalidConfiguration);
                    }
                }
                ConfigurationCheckpointPhase::Applied {
                    resource,
                    request_sha256,
                    ..
                } => {
                    if runtime_id != Some(&checkpoint.runtime_id)
                        || handed.get(resource).copied() != Some(request_sha256)
                        || !applied.insert(resource)
                    {
                        return Err(InvalidConfiguration);
                    }
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigurationResourceKind {
    Domain,
    Mailbox,
    Identity,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationResource {
    pub kind: ConfigurationResourceKind,
    pub id: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConfigurationCheckpointPhase {
    Bound {},
    Handoff {
        resource: ConfigurationResource,
        request_sha256: String,
    },
    Applied {
        resource: ConfigurationResource,
        request_sha256: String,
        #[serde(rename = "providerID")]
        provider_id: String,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationCheckpoint {
    pub lease: MailLease,
    #[serde(rename = "runtimeID")]
    pub runtime_id: String,
    pub phase: ConfigurationCheckpointPhase,
}
impl ConfigurationCheckpoint {
    pub fn validate(&self) -> Result<()> {
        self.lease.validate().map_err(|_| InvalidConfiguration)?;
        if !runtime(&self.runtime_id) {
            return Err(InvalidConfiguration);
        }
        match &self.phase {
            ConfigurationCheckpointPhase::Bound {} => {}
            ConfigurationCheckpointPhase::Handoff {
                resource,
                request_sha256,
            } => {
                if !id(&resource.id) || !hash(request_sha256) {
                    return Err(InvalidConfiguration);
                }
            }
            ConfigurationCheckpointPhase::Applied {
                resource,
                request_sha256,
                provider_id,
            } => {
                if !id(&resource.id) || !hash(request_sha256) || !provider(provider_id) {
                    return Err(InvalidConfiguration);
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationDkim {
    pub id: String,
    pub selector: String,
    pub public_key: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationProof {
    #[serde(rename = "runtimeID")]
    pub runtime_id: String,
    pub domains: BTreeMap<String, String>,
    pub mailboxes: BTreeMap<String, String>,
    pub identities: BTreeMap<String, Option<String>>,
    pub dkim: BTreeMap<String, Vec<ConfigurationDkim>>,
}
impl ConfigurationProof {
    /// Runtime ownership and cryptographic DKIM parsing are verified independently by Core.
    pub fn validate_for(&self, spec: &ConfigurationSpec) -> Result<()> {
        if self.domains.len() != spec.snapshot.domains.len()
            || self.mailboxes.len() != spec.snapshot.mailboxes.len()
            || self.identities.len() != spec.snapshot.identities.len()
            || self.dkim.len() != spec.snapshot.domains.len()
        {
            return Err(InvalidConfiguration);
        }
        let mut domains = BTreeSet::new();
        for desired in &spec.snapshot.domains {
            let observed = self.domains.get(&desired.id).ok_or(InvalidConfiguration)?;
            if !provider(observed)
                || !domains.insert(observed)
                || desired
                    .provider_id
                    .as_ref()
                    .is_some_and(|old| old != observed)
                || !self.dkim.contains_key(&desired.id)
            {
                return Err(InvalidConfiguration);
            }
        }
        let mut accounts = BTreeSet::new();
        for desired in &spec.snapshot.mailboxes {
            let observed = self
                .mailboxes
                .get(&desired.id)
                .ok_or(InvalidConfiguration)?;
            if !provider(observed)
                || !accounts.insert(observed)
                || desired
                    .provider_id
                    .as_ref()
                    .is_some_and(|old| old != observed)
            {
                return Err(InvalidConfiguration);
            }
        }
        for desired in &spec.snapshot.identities {
            match self
                .identities
                .get(&desired.id)
                .ok_or(InvalidConfiguration)?
            {
                None if !desired.enabled && desired.provider_id.is_none() => {}
                Some(observed)
                    if (desired.enabled || desired.provider_id.is_some())
                        && provider(observed)
                        && accounts.insert(observed)
                        && desired
                            .provider_id
                            .as_ref()
                            .is_none_or(|old| old == observed) => {}
                _ => return Err(InvalidConfiguration),
            }
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationCompletion {
    pub lease: MailLease,
    pub outcome: MailOutcome,
    pub proof: Option<ConfigurationProof>,
}
impl ConfigurationCompletion {
    pub fn validate(&self) -> Result<()> {
        self.lease.validate().map_err(|_| InvalidConfiguration)?;
        match (self.outcome, &self.proof) {
            (MailOutcome::NeedsReview, None) => Ok(()),
            (MailOutcome::Succeeded, Some(proof))
                if runtime(&proof.runtime_id)
                    && proof.domains.len() <= 32
                    && proof.mailboxes.len() <= 64
                    && proof.identities.len() <= 128
                    && proof.dkim.len() <= 32 =>
            {
                for mapping in [&proof.domains, &proof.mailboxes] {
                    if mapping
                        .iter()
                        .any(|(key, value)| !id(key) || !provider(value))
                    {
                        return Err(InvalidConfiguration);
                    }
                }
                if proof
                    .identities
                    .iter()
                    .any(|(key, value)| !id(key) || value.as_deref().is_some_and(|v| !provider(v)))
                {
                    return Err(InvalidConfiguration);
                }
                for (key, values) in &proof.dkim {
                    if !id(key)
                        || values.is_empty()
                        || values.len() > 2
                        || values.iter().any(|v| {
                            !provider(&v.id)
                                || v.selector.len() > 64
                                || !v.selector.bytes().all(|b| {
                                    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'
                                })
                                || !(128..=2048).contains(&v.public_key.len())
                                || !v
                                    .public_key
                                    .bytes()
                                    .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
                        })
                    {
                        return Err(InvalidConfiguration);
                    }
                }
                Ok(())
            }
            _ => Err(InvalidConfiguration),
        }
    }
}
