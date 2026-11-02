//! Retained source evidence lives inside the immutable Core-referenced artifact.
use crate::{
    Result,
    artifact::{Archive, MAX_OUTPUT, MAX_SOURCE},
    error,
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const DOMAIN: &[u8] = b"webdock.build.evidence.v1\0";
const DOMAIN_V2: &[u8] = b"webdock.build.evidence.v2\0";
const MAX_INVENTORY_BYTES: usize = 64 * 1024 * 1024;
const REQUIRED: [&str; 4] = [
    "image.tar",
    "source.tar",
    "provenance.json",
    "source.spdx.json",
];
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MachineIdentity {
    pub image_sha256: String,
    pub kernel_sha256: Option<String>,
    pub initrd_sha256: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildEvidence {
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    #[serde(rename = "buildID")]
    pub build_id: String,
    pub generation: i32,
    pub source_commit: String,
    pub recipe: String,
    pub root_directory: String,
    pub machine: MachineIdentity,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlobIdentity {
    pub sha256: String,
    pub size_bytes: u64,
}
impl BlobIdentity {
    fn of(bytes: &[u8]) -> Self {
        Self {
            sha256: hash(bytes),
            size_bytes: bytes.len() as u64,
        }
    }
    fn check(&self, bytes: &[u8]) -> Result<()> {
        if *self == Self::of(bytes) {
            Ok(())
        } else {
            Err(error("artifact_evidence_mismatch"))
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provenance {
    pub version: u8,
    pub build: BuildEvidence,
    pub source: BlobIdentity,
    pub image: BlobIdentity,
    pub image_digest: String,
    pub inventory: BlobIdentity,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontend: Option<FrontendEvidence>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrontendEvidence {
    pub output: BlobIdentity,
    pub configuration: BlobIdentity,
    pub directory: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Detached {
    algorithm: String,
    public_key: String,
    signature: String,
}
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn json_bytes(value: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| error("invalid_artifact_evidence"))
}
fn invalid() -> crate::Error {
    error("invalid_artifact_evidence")
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
impl BuildEvidence {
    fn validate(&self) -> Result<()> {
        for value in [&self.customer_id, &self.project_id, &self.build_id] {
            crate::contracts::id(value)?;
        }
        crate::contracts::sha(&self.source_commit)?;
        if self.generation <= 0
            || !["dockerfile", crate::frontend::RECIPE].contains(&self.recipe.as_str())
            || !valid_hash(&self.machine.image_sha256)
            || self.machine.kernel_sha256.is_some() != self.machine.initrd_sha256.is_some()
        {
            return Err(invalid());
        }
        for pin in [&self.machine.kernel_sha256, &self.machine.initrd_sha256]
            .into_iter()
            .flatten()
        {
            if !valid_hash(pin) {
                return Err(invalid());
            }
        }
        if self.root_directory != "." {
            crate::artifact::safe_path(&self.root_directory)?;
        }
        Ok(())
    }
}
fn inventory(source: &Archive, build: &BuildEvidence, created: &str, source_hash: &str) -> Value {
    let files:Vec<_>=source.files().iter().enumerate().map(|(i,(path,bytes))|json!({"SPDXID":format!("SPDXRef-SourceFile-{i}"),"fileName":format!("./{path}"),"checksums":[{"algorithm":"SHA256","checksumValue":hash(bytes)}],"licenseConcluded":"NOASSERTION","licenseInfoInFiles":["NOASSERTION"],"copyrightText":"NOASSERTION"})).collect();
    let relationships:Vec<_>=(0..files.len()).map(|i|json!({"spdxElementId":"SPDXRef-DOCUMENT","relationshipType":"DESCRIBES","relatedSpdxElement":format!("SPDXRef-SourceFile-{i}")})).collect();
    json!({"spdxVersion":"SPDX-2.3","dataLicense":"CC0-1.0","SPDXID":"SPDXRef-DOCUMENT","name":"Normalized build source file inventory","documentNamespace":format!("https://webdock.dev/spdx/build/{}/{}/{}/{}",build.customer_id,build.build_id,build.generation,source_hash),"creationInfo":{"creators":["Tool: webdock-build-worker"],"created":created,"comment":"Source-file hashes only. Operating-system and dependency packages have not been resolved."},"files":files,"relationships":relationships})
}
fn signing_message(provenance: &[u8], version: u8) -> Vec<u8> {
    let mut data = if version == 2 { DOMAIN_V2 } else { DOMAIN }.to_vec();
    data.extend(Sha256::digest(provenance));
    data
}
/// Only the trusted worker adds evidence; guest output contains only recipe-selected bytes.
pub fn assemble(
    guest: &[u8],
    source: &[u8],
    build: &BuildEvidence,
    signer: Option<&SigningKey>,
) -> Result<Vec<u8>> {
    build.validate()?;
    let mut files = Archive::read(guest, MAX_OUTPUT)?.into_files();
    let frontend_recipe = build.recipe == crate::frontend::RECIPE;
    if !files.contains_key("image.tar")
        || (frontend_recipe && (files.len() != 2 || !files.contains_key("frontend.tar")))
        || (!frontend_recipe && files.len() != 1)
    {
        return Err(invalid());
    }
    let image = &files["image.tar"];
    if files.values().map(|v| v.len() as u64).sum::<u64>() + source.len() as u64 > MAX_OUTPUT {
        return Err(error("artifact_limit"));
    }
    let image_digest = crate::artifact::validate_oci(image)?;
    let parsed_source = Archive::read(source, MAX_SOURCE)?;
    let frontend = if frontend_recipe {
        let (config, configuration) =
            crate::frontend::FrontendConfig::from_source(&parsed_source, &build.root_directory)?;
        crate::frontend::validate_output(&files["frontend.tar"])?;
        Some(FrontendEvidence {
            output: BlobIdentity::of(&files["frontend.tar"]),
            configuration: BlobIdentity::of(configuration),
            directory: config.directory,
        })
    } else {
        None
    };
    let created = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let source_identity = BlobIdentity::of(source);
    let inventory = json_bytes(&inventory(
        &parsed_source,
        build,
        &created,
        &source_identity.sha256,
    ))?;
    if inventory.len() > MAX_INVENTORY_BYTES {
        return Err(error("artifact_inventory_limit"));
    }
    let provenance = Provenance {
        version: if frontend_recipe { 2 } else { 1 },
        build: build.clone(),
        source: source_identity,
        image: BlobIdentity::of(image),
        image_digest,
        inventory: BlobIdentity::of(&inventory),
        created_at: created,
        frontend,
    };
    let version = provenance.version;
    let provenance = json_bytes(&provenance)?;
    if let Some(key) = signer {
        files.insert(
            "signature.json".into(),
            json_bytes(&Detached {
                algorithm: "Ed25519".into(),
                public_key: hex::encode(key.verifying_key().to_bytes()),
                signature: hex::encode(key.sign(&signing_message(&provenance, version)).to_bytes()),
            })?,
        );
    }
    files.insert("source.tar".into(), source.to_vec());
    files.insert("source.spdx.json".into(), inventory);
    files.insert("provenance.json".into(), provenance);
    let bytes = Archive::from_files(files)?.encode()?;
    if bytes.len() as u64 > MAX_OUTPUT {
        return Err(error("artifact_limit"));
    }
    Ok(bytes)
}
/// `expected_signer` is an external trust pin; an embedded public key is never an organization trust decision.
pub fn validate(
    archive: &Archive,
    customer: &str,
    build: &str,
    generation: i32,
    expected_signer: Option<&VerifyingKey>,
) -> Result<Option<Provenance>> {
    let files = archive.files();
    if files.len() == 1 && files.contains_key("image.tar") {
        if expected_signer.is_some() {
            return Err(error("artifact_signature_required"));
        }
        crate::artifact::validate_oci(&files["image.tar"])?;
        return Ok(None);
    }
    let frontend = files.contains_key("frontend.tar");
    let required_count = REQUIRED.len() + usize::from(frontend);
    if ![required_count, required_count + 1].contains(&files.len())
        || REQUIRED.iter().any(|name| !files.contains_key(*name))
        || files.keys().any(|name| {
            !REQUIRED.contains(&name.as_str()) && name != "signature.json" && name != "frontend.tar"
        })
        || files["provenance.json"].len() > 16384
        || files["source.spdx.json"].len() > MAX_INVENTORY_BYTES
    {
        return Err(invalid());
    }
    let provenance: Provenance =
        serde_json::from_slice(&files["provenance.json"]).map_err(|_| invalid())?;
    provenance.build.validate()?;
    if provenance.version != if frontend { 2 } else { 1 }
        || (provenance.build.recipe == crate::frontend::RECIPE) != frontend
        || provenance.frontend.is_some() != frontend
        || provenance.build.customer_id != customer
        || provenance.build.build_id != build
        || provenance.build.generation != generation
        || chrono::DateTime::parse_from_rfc3339(&provenance.created_at).is_err()
    {
        return Err(error("artifact_evidence_scope_mismatch"));
    }
    provenance.source.check(&files["source.tar"])?;
    provenance.image.check(&files["image.tar"])?;
    provenance.inventory.check(&files["source.spdx.json"])?;
    if crate::artifact::validate_oci(&files["image.tar"])? != provenance.image_digest {
        return Err(error("artifact_evidence_mismatch"));
    }
    let source = Archive::read(&files["source.tar"], MAX_SOURCE)?;
    if let Some(frontend) = &provenance.frontend {
        frontend.output.check(&files["frontend.tar"])?;
        crate::frontend::validate_output(&files["frontend.tar"])?;
        let (config, bytes) = crate::frontend::FrontendConfig::from_source(
            &source,
            &provenance.build.root_directory,
        )?;
        frontend.configuration.check(bytes)?;
        if frontend.directory != config.directory {
            return Err(error("artifact_evidence_mismatch"));
        }
    }
    let actual: Value =
        serde_json::from_slice(&files["source.spdx.json"]).map_err(|_| invalid())?;
    if actual
        != inventory(
            &source,
            &provenance.build,
            &provenance.created_at,
            &provenance.source.sha256,
        )
    {
        return Err(error("artifact_inventory_mismatch"));
    }
    if let Some(bytes) = files.get("signature.json") {
        if bytes.len() > 1024 {
            return Err(invalid());
        }
        let signature: Detached = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        let public: [u8; 32] = hex::decode(&signature.public_key)
            .map_err(|_| invalid())?
            .try_into()
            .map_err(|_| invalid())?;
        let public = VerifyingKey::from_bytes(&public).map_err(|_| invalid())?;
        if signature.algorithm != "Ed25519" || expected_signer.is_some_and(|pin| *pin != public) {
            return Err(error("artifact_signer_mismatch"));
        }
        let signature =
            Signature::from_slice(&hex::decode(&signature.signature).map_err(|_| invalid())?)
                .map_err(|_| invalid())?;
        public
            .verify_strict(
                &signing_message(&files["provenance.json"], provenance.version),
                &signature,
            )
            .map_err(|_| error("artifact_signature_invalid"))?;
    } else if expected_signer.is_some() {
        return Err(error("artifact_signature_required"));
    }
    Ok(Some(provenance))
}
pub(crate) fn validate_for_store(
    archive: &Archive,
    customer: &str,
    build: &str,
    generation: i32,
) -> Result<String> {
    if archive.files().len() == 1 {
        return crate::artifact::validate_oci(
            archive.files().get("image.tar").ok_or_else(invalid)?,
        );
    }
    Ok(validate(archive, customer, build, generation, None)?
        .ok_or_else(invalid)?
        .image_digest)
}
