//! Exact-byte catalog release verification. Trust keys are selected by the operator catalog.
use crate::{ErrorCode, Manifest, PluginError, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy)]
pub struct ReleaseInput<'a> {
    pub registry_url: &'a str,
    pub id: &'a str,
    pub version: &'a str,
    pub kid: &'a str,
    pub key: &'a VerifyingKey,
    pub jws: &'a str,
    pub manifest_raw: &'a str,
    pub descriptor: &'a str,
}
pub struct VerifiedRelease {
    pub manifest: Manifest,
    pub manifest_sha256: String,
    pub signer_sha256: String,
}
fn invalid() -> PluginError {
    PluginError::new(ErrorCode::InvalidInput, "Invalid signed plugin release")
}
pub fn strict_json(bytes: &[u8]) -> Result<Value> {
    crate::manifest::strict_json(bytes)
}
pub fn sha256(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}
pub fn verify_release(input: &ReleaseInput<'_>) -> Result<VerifiedRelease> {
    if input.manifest_raw.len() > 262144
        || input.descriptor.len() > 262144
        || input.jws.len() > 65536
    {
        return Err(invalid());
    }
    let segments: Vec<_> = input.jws.split('.').collect();
    if segments.len() != 3 {
        return Err(invalid());
    }
    let header = strict_json(&B64.decode(segments[0]).map_err(|_| invalid())?)?;
    let h = header.as_object().ok_or_else(invalid)?;
    if header["alg"] != "EdDSA"
        || header["kid"] != input.kid
        || h.keys()
            .any(|key| !["alg", "kid", "typ"].contains(&key.as_str()))
    {
        return Err(invalid());
    }
    let signature = Signature::from_slice(&B64.decode(segments[2]).map_err(|_| invalid())?)
        .map_err(|_| invalid())?;
    let signed = format!("{}.{}", segments[0], segments[1]);
    input
        .key
        .verify_strict(signed.as_bytes(), &signature)
        .map_err(|_| invalid())?;
    let payload = strict_json(&B64.decode(segments[1]).map_err(|_| invalid())?)?;
    if payload["v"] != 1
        || payload["registry"] != input.registry_url
        || payload["plugin_slug"] != input.id
        || payload["release_version"] != input.version
        || payload["kid"] != input.kid
    {
        return Err(invalid());
    }
    let digest = sha256(input.manifest_raw);
    let asset_name = format!("{}-universal.json", input.id);
    let assets = payload["assets"].as_array().ok_or_else(invalid)?;
    let assets: Vec<_> = assets.iter().filter(|a| a["name"] == asset_name).collect();
    if assets.len() != 1
        || payload["manifest_sha256"] != digest
        || input.manifest_raw != input.descriptor
        || assets[0]["sha256"] != digest
        || assets[0]["size"].as_u64() != Some(input.descriptor.len() as u64)
    {
        return Err(invalid());
    }
    let manifest = Manifest::parse(input.manifest_raw.as_bytes())?;
    if manifest.id() != input.id
        || manifest.version() != input.version
        || manifest.as_json()["image"]
            .as_str()
            .is_none_or(|image| image.ends_with(&format!("@sha256:{}", "0".repeat(64))))
    {
        return Err(invalid());
    }
    Ok(VerifiedRelease {
        manifest,
        manifest_sha256: digest,
        signer_sha256: sha256(input.key.as_bytes()),
    })
}
