use std::io::{self, Read, Write};
use webdock_plugin_protocol::Manifest;

const MAX_REGISTRY_BYTES: usize = 64 * 1024;

fn validate(bytes: &[u8], authoring: bool) -> Result<(), &'static str> {
    if bytes.len() > MAX_REGISTRY_BYTES {
        return Err("Manifest exceeds registry 64 KiB limit");
    }
    let manifest = Manifest::parse(bytes).map_err(|_| "Invalid native-v2 manifest")?;
    if !authoring
        && manifest.as_json()["image"]
            .as_str()
            .is_none_or(|image| image.ends_with(&format!("@sha256:{}", "0".repeat(64))))
    {
        return Err("Nonzero immutable image digest required");
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let authoring = match arguments.as_slice() {
        [] => false,
        [argument] if argument == "--authoring" => true,
        _ => return Err("Usage: manifest-validator [--authoring] < manifest.json".into()),
    };
    let mut bytes = Vec::new();
    io::stdin()
        .take((MAX_REGISTRY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    validate(&bytes, authoring)?;
    // Preserve the exact release bytes; never normalize a signed descriptor.
    io::stdout().write_all(&bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::path::Path;

    fn fixture(name: &str) -> Value {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../plugins");
        let mut value: Value =
            serde_json::from_slice(&std::fs::read(root.join(name).join("manifest.json")).unwrap())
                .unwrap();
        value["image"] = json!(format!("ghcr.io/spitzli/{name}@sha256:{}", "a".repeat(64)));
        value
    }

    #[test]
    fn accepts_actual_native_product_manifests_with_finalized_digest() {
        for name in [
            "echo-demo",
            "hosting",
            "mail",
            "archives",
            "object-storage",
            "postgres",
        ] {
            assert!(
                validate(&serde_json::to_vec(&fixture(name)).unwrap(), false).is_ok(),
                "{name}"
            );
        }
    }

    #[test]
    fn rejects_invalid_release_and_privilege_boundaries() {
        let original = fixture("echo-demo");
        for (key, value) in [
            ("protocolVersion", json!(1)),
            ("kind", json!("provider")),
            ("sdkVersion", json!(1)),
            ("dataProtocolVersion", json!(1)),
            ("image", json!("ghcr.io/spitzli/echo-demo:latest")),
            (
                "image",
                json!(format!(
                    "ghcr.io/spitzli/echo-demo@sha256:{}",
                    "0".repeat(64)
                )),
            ),
            ("privileged", json!(true)),
            ("capabilities", json!(["plugin.root"])),
            ("egress", json!([{"host":"*","port":443}])),
            (
                "services",
                json!({"required":[{"name":"root","version":1}],"optional":[]}),
            ),
            ("workloads", json!({"kinds":["v1/Node"]})),
            (
                "configuration",
                json!({"type":"object","properties":{"a":{"type":"object","properties":{"b":{"type":"object","properties":{"c":{"type":"object","properties":{"d":{"type":"string"}}}}}}}}}),
            ),
        ] {
            let mut changed = original.clone();
            changed[key] = value;
            assert!(
                validate(&serde_json::to_vec(&changed).unwrap(), false).is_err(),
                "{key}"
            );
        }
        assert!(validate(&vec![b' '; MAX_REGISTRY_BYTES + 1], false).is_err());
        let mut duplicate = serde_json::to_vec(&original).unwrap();
        duplicate.pop();
        duplicate.extend_from_slice(b",\"sdkVersion\":2}");
        assert!(validate(&duplicate, false).is_err());
    }

    #[test]
    fn only_explicit_authoring_mode_allows_placeholder_digest() {
        let mut authoring = fixture("echo-demo");
        authoring["image"] = json!(format!(
            "ghcr.io/spitzli/echo-demo@sha256:{}",
            "0".repeat(64)
        ));
        let bytes = serde_json::to_vec(&authoring).unwrap();
        assert!(validate(&bytes, false).is_err());
        assert!(validate(&bytes, true).is_ok());
        authoring["sdkVersion"] = json!(1);
        assert!(validate(&serde_json::to_vec(&authoring).unwrap(), true).is_err());
    }
}
