use serde_json::json;
use webdock_plugin_host::catalog::Catalog;
#[path = "support/catalog_fixture.rs"]
mod catalog_fixture;
use catalog_fixture::catalog;

#[test]
fn signed_catalog_accepts_required_native_jobs_service() {
    required_native_service("jobs");
}
#[test]
fn signed_catalog_accepts_required_native_storage_service() {
    required_native_service("storage");
}
fn required_native_service(service: &str) {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    let mut value = catalog();
    let mut manifest: serde_json::Value =
        serde_json::from_str(value["releases"][0]["manifestRaw"].as_str().unwrap()).unwrap();
    if !manifest["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("plugin.data.write"))
    {
        manifest["capabilities"]
            .as_array_mut()
            .unwrap()
            .push(json!("plugin.data.write"));
    }
    manifest["jobs"] = json!([{"name":"maintenance","capabilities":["plugin.data.write"]}]);
    manifest["services"]["required"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":service,"version":1}));
    let raw = manifest.to_string();
    let hash = hex::encode(Sha256::digest(raw.as_bytes()));
    let payload = json!({"v":1,"registry":"https://registry.example.test","plugin_slug":"postgres","release_version":"0.1.0","kid":"trusted","manifest_sha256":hash,"assets":[{"name":"postgres-universal.json","sha256":hash,"size":raw.len()}]});
    let body = format!(
        "{}.{}",
        B64.encode(r#"{"alg":"EdDSA","kid":"trusted"}"#),
        B64.encode(payload.to_string())
    );
    value["releases"][0]["jws"] = json!(format!(
        "{body}.{}",
        B64.encode(
            SigningKey::from_bytes(&[27; 32])
                .sign(body.as_bytes())
                .to_bytes()
        )
    ));
    value["releases"][0]["manifestRaw"] = json!(raw);
    value["releases"][0]["descriptor"] = value["releases"][0]["manifestRaw"].clone();
    let admitted = Catalog::from_bytes(&serde_json::to_vec(&value).unwrap())
        .expect("verified immediate jobs release");
    assert_eq!(admitted.descriptors().len(), 1);
}

#[test]
fn review_binds_private_recipe_target_and_trusted_key_without_disclosing_them() {
    let raw = catalog();
    let catalog = Catalog::from_bytes(&serde_json::to_vec(&raw).unwrap()).unwrap();
    let public = serde_json::to_value(catalog.descriptors()).unwrap();
    assert_eq!(public[0]["targets"][0]["id"], "local-eu");
    assert!(!public.to_string().contains("private"));
    for mutate in ["recipe", "target"] {
        let mut changed = raw.clone();
        if mutate == "recipe" {
            changed["releases"][0]["recipe"]["profile"] = json!("changed");
        } else {
            changed["targets"][0]["policy"]["secretRef"] = json!("different");
        }
        let changed = Catalog::from_bytes(&serde_json::to_vec(&changed).unwrap()).unwrap();
        assert_ne!(
            public[0]["targets"][0]["reviewSha256"],
            serde_json::to_value(changed.descriptors()).unwrap()[0]["targets"][0]["reviewSha256"]
        );
    }
}
#[test]
fn catalog_rejects_untrusted_release_duplicate_identity_and_feature_invention() {
    let raw = catalog();
    for case in 0..5 {
        let mut value = raw.clone();
        match case {
            0 => value["keys"][0]["kid"] = json!("other"),
            1 => {
                let release = value["releases"][0].clone();
                value["releases"].as_array_mut().unwrap().push(release);
            }
            2 => value["releases"][0]["supportedFeatures"] = json!(["invented"]),
            3 => value["releases"][0]["targetIDs"] = json!(["missing"]),
            _ => value["keys"][0]["key"]["d"] = json!("private"),
        }
        assert!(
            Catalog::from_bytes(&serde_json::to_vec(&value).unwrap()).is_err(),
            "case {case}"
        );
    }
    let bytes = raw.to_string().replacen('{', "{\"version\":1,", 1);
    assert!(Catalog::from_bytes(bytes.as_bytes()).is_err());
}
