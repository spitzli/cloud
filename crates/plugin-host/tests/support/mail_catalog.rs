//! Signed Mail fixture; the image is a test identity, never an OCI or delivery claim.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn catalog(policy: Value, recipe: Value) -> Value {
    let key = SigningKey::from_bytes(&[29; 32]);
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../plugins/mail/manifest.json")).unwrap();
    manifest["image"] = json!(format!("fixture.invalid/mail@sha256:{}", "a".repeat(64)));
    let version = manifest["version"].as_str().unwrap().to_owned();
    let raw = manifest.to_string();
    let hash = hex::encode(Sha256::digest(raw.as_bytes()));
    let payload = json!({"v":1,"registry":"https://registry.example.test","plugin_slug":"mail","release_version":version,"kid":"mail-test","manifest_sha256":hash,"assets":[{"name":"mail-universal.json","sha256":hash,"size":raw.len()}]});
    let body = format!(
        "{}.{}",
        B64.encode(r#"{"alg":"EdDSA","kid":"mail-test"}"#),
        B64.encode(payload.to_string())
    );
    json!({"version":1,"platformID":"fixture","keys":[{"registryURL":"https://registry.example.test","kid":"mail-test","key":{"kty":"OKP","crv":"Ed25519","x":B64.encode(key.verifying_key().as_bytes())}}],"targets":[{"id":"mail-eu","label":"Owned Mail fixture","clusterID":"42","policy":policy}],"releases":[{"id":"mail","version":version,"registryURL":"https://registry.example.test","kid":"mail-test","jws":format!("{body}.{}",B64.encode(key.sign(body.as_bytes()).to_bytes())),"manifestRaw":raw,"descriptor":raw,"recipe":recipe,"supportedFeatures":["add-domains","create-mailboxes","manage-incoming"],"targetIDs":["mail-eu"]}]})
}

pub fn recipe(mailboxes: Value) -> Value {
    json!({"storage":true,"sealingKey":true,"routeDomains":["mail.example.test","mail-admin.example.test"],"privateConfiguration":{"platformTransport":{"kind":"smtp","host":"127.0.0.1","port":9},"mailboxes":mailboxes}})
}
