use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use webdock_plugin_protocol::release::{ReleaseInput, verify_release};

fn hash(raw: &str) -> String {
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}
fn signed(payload: &str, header: &str) -> String {
    let body = format!("{}.{}", B64.encode(header), B64.encode(payload));
    format!(
        "{body}.{}",
        B64.encode(
            SigningKey::from_bytes(&[19; 32])
                .sign(body.as_bytes())
                .to_bytes()
        )
    )
}
fn fixture() -> (String, Value) {
    let raw = include_str!("fixtures/project-notes.json").to_string();
    let m: Value = serde_json::from_str(&raw).unwrap();
    let payload = json!({"v":1,"registry":"https://registry.example.test","plugin_slug":m["name"],"release_version":m["version"],"kid":"trusted","manifest_sha256":hash(&raw),"assets":[{"name":format!("{}-universal.json",m["name"].as_str().unwrap()),"sha256":hash(&raw),"size":raw.len()}]});
    (raw, payload)
}
#[test]
fn exact_signed_descriptor_and_identity_are_required() {
    let (raw, payload) = fixture();
    let key = SigningKey::from_bytes(&[19; 32]).verifying_key();
    let jws = signed(&payload.to_string(), r#"{"alg":"EdDSA","kid":"trusted"}"#);
    let input = ReleaseInput {
        registry_url: "https://registry.example.test",
        id: payload["plugin_slug"].as_str().unwrap(),
        version: payload["release_version"].as_str().unwrap(),
        kid: "trusted",
        key: &key,
        jws: &jws,
        manifest_raw: &raw,
        descriptor: &raw,
    };
    let verified = verify_release(&input).unwrap();
    assert_eq!(verified.manifest.as_bytes(), raw.as_bytes());
    assert_eq!(verified.manifest_sha256, hash(&raw));
    for changed in [
        ReleaseInput {
            descriptor: &format!("{raw} "),
            ..input
        },
        ReleaseInput {
            registry_url: "https://wrong.test",
            ..input
        },
        ReleaseInput {
            version: "9.9.9",
            ..input
        },
        ReleaseInput {
            kid: "untrusted",
            ..input
        },
        ReleaseInput {
            jws: &format!("{jws}x"),
            ..input
        },
    ] {
        assert!(verify_release(&changed).is_err());
    }
}
#[test]
fn signed_duplicates_critical_jose_and_placeholder_images_are_rejected() {
    let (raw, mut payload) = fixture();
    let key = SigningKey::from_bytes(&[19; 32]).verifying_key();
    let run = |raw: &str, payload: &str, header: &str| {
        let jws = signed(payload, header);
        verify_release(&ReleaseInput {
            registry_url: "https://registry.example.test",
            id: "project-notes",
            version: "1.0.0",
            kid: "trusted",
            key: &key,
            jws: &jws,
            manifest_raw: raw,
            descriptor: raw,
        })
        .is_err()
    };
    let header = r#"{"alg":"EdDSA","kid":"trusted"}"#;
    assert!(!run(&raw, &payload.to_string(), header));
    assert!(run(
        &raw,
        &payload.to_string().replacen('{', "{\"v\":1,", 1),
        header
    ));
    for h in [
        r#"{"alg":"EdDSA","kid":"trusted","crit":["unknown"],"unknown":true}"#,
        r#"{"alg":"EdDSA","kid":"trusted","b64":false}"#,
        r#"{"alg":"EdDSA","kid":"wrong","kid":"trusted"}"#,
    ] {
        assert!(run(&raw, &payload.to_string(), h));
    }
    let duplicate = raw.replacen('{', "{\"name\":\"other\",", 1);
    payload["manifest_sha256"] = json!(hash(&duplicate));
    payload["assets"][0]["sha256"] = json!(hash(&duplicate));
    payload["assets"][0]["size"] = json!(duplicate.len());
    assert!(run(&duplicate, &payload.to_string(), header));
    let mut m: Value = serde_json::from_str(&raw).unwrap();
    m["image"] = json!(format!(
        "registry.example.test/plugin@sha256:{}",
        "0".repeat(64)
    ));
    let zero = m.to_string();
    payload["manifest_sha256"] = json!(hash(&zero));
    payload["assets"][0]["sha256"] = json!(hash(&zero));
    payload["assets"][0]["size"] = json!(zero.len());
    assert!(run(&zero, &payload.to_string(), header));
}

#[test]
fn verifies_compact_jws_generated_by_node_crypto() {
    let f: Value = serde_json::from_str(include_str!("fixtures/release-node.json")).unwrap();
    let bytes: [u8; 32] = B64
        .decode(f["key"]["x"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let key = ed25519_dalek::VerifyingKey::from_bytes(&bytes).unwrap();
    let verified = verify_release(&ReleaseInput {
        registry_url: f["registryURL"].as_str().unwrap(),
        id: f["id"].as_str().unwrap(),
        version: f["version"].as_str().unwrap(),
        kid: f["kid"].as_str().unwrap(),
        key: &key,
        jws: f["jws"].as_str().unwrap(),
        manifest_raw: f["manifestRaw"].as_str().unwrap(),
        descriptor: f["descriptor"].as_str().unwrap(),
    })
    .unwrap();
    assert_eq!(
        verified.manifest_sha256,
        f["manifestSha256"].as_str().unwrap()
    );
}

#[test]
fn signed_asset_and_payload_changes_and_resource_bounds_fail_closed() {
    let (raw, payload) = fixture();
    let key = SigningKey::from_bytes(&[19; 32]).verifying_key();
    let header = r#"{"alg":"EdDSA","kid":"trusted"}"#;
    for case in 0..7 {
        let mut changed = payload.clone();
        match case {
            0 => changed["v"] = json!(2),
            1 => changed["plugin_slug"] = json!("other"),
            2 => changed["kid"] = json!("other"),
            3 => changed["manifest_sha256"] = json!("0".repeat(64)),
            4 => changed["assets"][0]["size"] = json!(raw.len() + 1),
            5 => changed["assets"][0]["sha256"] = json!("0".repeat(64)),
            _ => changed["assets"][0]["name"] = json!("wrong-universal.json"),
        }
        let jws = signed(&changed.to_string(), header);
        assert!(
            verify_release(&ReleaseInput {
                registry_url: "https://registry.example.test",
                id: "project-notes",
                version: "1.0.0",
                kid: "trusted",
                key: &key,
                jws: &jws,
                manifest_raw: &raw,
                descriptor: &raw
            })
            .is_err(),
            "case {case}"
        );
    }
    let jws = signed(&payload.to_string(), header);
    let input = ReleaseInput {
        registry_url: "https://registry.example.test",
        id: "project-notes",
        version: "1.0.0",
        kid: "trusted",
        key: &key,
        jws: &jws,
        manifest_raw: &raw,
        descriptor: &raw,
    };
    let oversized = " ".repeat(262145);
    assert!(
        verify_release(&ReleaseInput {
            manifest_raw: &oversized,
            ..input
        })
        .is_err()
    );
    assert!(
        verify_release(&ReleaseInput {
            descriptor: &oversized,
            ..input
        })
        .is_err()
    );
    assert!(
        verify_release(&ReleaseInput {
            jws: &"x".repeat(65537),
            ..input
        })
        .is_err()
    );
    let wrong_key = SigningKey::from_bytes(&[21; 32]).verifying_key();
    assert!(
        verify_release(&ReleaseInput {
            key: &wrong_key,
            ..input
        })
        .is_err()
    );
}
