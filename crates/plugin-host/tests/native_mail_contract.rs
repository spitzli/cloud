use webdock_plugin_host::{MailRuntimeMode, MailRuntimeProof};
#[test]
fn native_runtime_modes_separate_new_work_from_bookkeeping() {
    assert!(MailRuntimeMode::Admission.permits("enabled"));
    assert!(!MailRuntimeMode::Admission.permits("draining"));
    assert!(MailRuntimeMode::Dispatch.permits("draining"));
    assert!(!MailRuntimeMode::Dispatch.permits("disabled"));
    assert!(MailRuntimeMode::Bookkeeping.permits("disabled"));
    assert!(!MailRuntimeMode::Bookkeeping.permits("candidate"));
    assert!(std::mem::size_of::<MailRuntimeProof>() > 0);
}

#[allow(
    dead_code,
    reason = "Signed catalog fixture also exposes unrelated release-history builders"
)]
#[path = "support/catalog_fixture.rs"]
mod catalog_fixture;
#[test]
fn native_mail_service_is_not_advertised_to_other_plugins() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    use ed25519_dalek::Signer;
    let mut catalog = catalog_fixture::catalog();
    let release = &mut catalog["releases"][0];
    let mut manifest: serde_json::Value =
        serde_json::from_str(release["manifestRaw"].as_str().unwrap()).unwrap();
    manifest["services"]["required"] =
        serde_json::json!([{"name":"native-mail-control","version":1}]);
    manifest["services"]["optional"] = serde_json::json!([]);
    let raw = manifest.to_string();
    let hash = webdock_plugin_protocol::release::sha256(raw.as_bytes());
    let segments = release["jws"]
        .as_str()
        .unwrap()
        .split('.')
        .collect::<Vec<_>>();
    let header = segments[0].to_owned();
    let mut payload: serde_json::Value =
        serde_json::from_slice(&B64.decode(segments[1]).unwrap()).unwrap();
    payload["manifest_sha256"] = serde_json::json!(hash);
    payload["assets"][0]["sha256"] = serde_json::json!(hash);
    payload["assets"][0]["size"] = serde_json::json!(raw.len());
    let body = format!("{header}.{}", B64.encode(payload.to_string()));
    release["manifestRaw"] = serde_json::json!(raw);
    release["descriptor"] = release["manifestRaw"].clone();
    release["jws"] = serde_json::json!(format!(
        "{body}.{}",
        B64.encode(
            ed25519_dalek::SigningKey::from_bytes(&[27; 32])
                .sign(body.as_bytes())
                .to_bytes()
        )
    ));
    assert!(
        webdock_plugin_host::Catalog::from_bytes(&serde_json::to_vec(&catalog).unwrap()).is_err(),
        "fixed Mail control is usable only by enrolled Mail"
    );
}
