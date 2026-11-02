//! Signed catalog contract remains a safe default test.
//! The retired generic process proof is replaced by the actual pinned, NativeControl-configured
//! mail_outbox_native::generic_installer_persisted_six_files_run_actual_mail_with_automatic_binding.
//! That replacement proves admitted Host/Worker material preparation and host startup, not rollout.
#[path = "support/mail_catalog.rs"]
mod mail_catalog;
use serde_json::{Value, json};
use webdock_plugin_host::Catalog;

#[test]
fn actual_signed_mail_catalog_binds_private_recipe_and_current_feature_ceiling() {
    let raw = mail_catalog::catalog(json!({}), mail_catalog::recipe(Value::Null));
    let catalog = Catalog::from_bytes(&serde_json::to_vec(&raw).unwrap()).unwrap();
    let public = serde_json::to_value(catalog.descriptors()).unwrap();
    assert_eq!(public[0]["pluginID"], "mail");
    assert_eq!(public[0]["version"], "0.3.0");
    assert_eq!(public[0]["schemaVersion"], 2);
    let manifest = webdock_plugin_protocol::Manifest::parse(
        raw["releases"][0]["manifestRaw"]
            .as_str()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(
        manifest.as_json()["services"],
        json!({"required":[{"name":"native-mail-control","version":1}],"optional":[]})
    );
    assert!(manifest.require_host_services(&[]).is_err());
    manifest
        .require_host_services(&[("native-mail-control", 1)])
        .unwrap();
    assert_eq!(public[0]["readableSchema"], json!({"min":1,"max":2}));
    assert_eq!(
        public[0]["supportedFeatures"],
        json!(["add-domains", "create-mailboxes", "manage-incoming"])
    );
    assert_eq!(public[0]["capabilities"], json!(["plugin.workloads"]));
    assert!(public[0].get("recipe").is_none());
    assert!(!public.to_string().contains("platformTransport"));
    for fault in ["recipe", "feature", "signature", "image"] {
        let mut changed = raw.clone();
        match fault {
            "recipe" => {
                changed["releases"][0]["recipe"]["privateConfiguration"]["trustUnverifiedDomains"] =
                    json!(true)
            }
            "feature" => changed["releases"][0]["supportedFeatures"] = json!(["create-mailboxes"]),
            "signature" => changed["releases"][0]["jws"] = json!("invalid.signature.input"),
            _ => {
                let text = changed["releases"][0]["manifestRaw"]
                    .as_str()
                    .unwrap()
                    .replace("fixture.invalid/mail", "changed.invalid/mail");
                changed["releases"][0]["manifestRaw"] = json!(text);
            }
        }
        let result = Catalog::from_bytes(&serde_json::to_vec(&changed).unwrap());
        if ["recipe", "feature"].contains(&fault) {
            assert_ne!(
                public[0]["targets"][0]["reviewSha256"],
                serde_json::to_value(result.unwrap().descriptors()).unwrap()[0]["targets"][0]["reviewSha256"]
            );
        } else {
            assert!(result.is_err());
        }
    }
}
