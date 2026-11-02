use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::Command,
};
use webdock_quality::architecture::{self, Package, Zone};
fn package(name: &str, zone: Zone, dependencies: &[&str]) -> Package {
    Package {
        name: name.into(),
        zone,
        dependencies: dependencies.iter().map(|s| (*s).into()).collect(),
    }
}
#[test]
fn graph_checks_transitive_boundaries_and_permits_shared_protocols() {
    let mut graph = BTreeMap::from([
        ("core".into(), package("core", Zone::Platform, &["bridge"])),
        ("bridge".into(), package("bridge", Zone::Other, &["vendor"])),
        (
            "vendor".into(),
            package("vendor", Zone::Provider, &["wire"]),
        ),
        ("wire".into(), package("wire", Zone::Shared, &[])),
    ]);
    assert_eq!(
        architecture::violations(&graph),
        ["provider boundary: core -> bridge -> vendor"]
    );
    graph.get_mut("bridge").unwrap().dependencies = BTreeSet::from(["wire".into()]);
    assert!(architecture::violations(&graph).is_empty());
    graph.get_mut("wire").unwrap().dependencies = BTreeSet::from(["core".into()]);
    let found = architecture::violations(&graph);
    assert!(found.iter().any(|s| s.contains("vendor -> wire -> core")));
    assert!(found.iter().any(|s| s.contains("wire -> core")));
}
fn write(root: &Path, name: &str, extra: &str) {
    let directory = root.join(name);
    std::fs::create_dir_all(directory.join("src")).unwrap();
    let package = directory.file_name().unwrap().to_str().unwrap();
    std::fs::write(
        directory.join("Cargo.toml"),
        format!("[package]\nname=\"{package}\"\nversion=\"0.1.0\"\nedition=\"2024\"\n{extra}\n"),
    )
    .unwrap();
    std::fs::write(directory.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n").unwrap();
}
fn lock(path: &Path) {
    let result = Command::new("cargo")
        .args(["generate-lockfile", "--offline", "--manifest-path"])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn actual_cargo_metadata_catches_renamed_optional_target_and_build_edges() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::write(root.join("Cargo.toml"),"[workspace]\nmembers=[\"crates/*\"]\nexclude=[\"plugins/vendor\",\"plugins/allowed\"]\nresolver=\"3\"\n").unwrap();
    write(root, "plugins/vendor", "[workspace]");
    write(
        root,
        "crates/worker",
        "[build-dependencies]\nrenamed={package=\"vendor\",path=\"../../plugins/vendor\"}",
    );
    write(
        root,
        "crates/bridge",
        "[target.'cfg(windows)'.dependencies]\nrenamed={package=\"vendor\",path=\"../../plugins/vendor\",optional=true}",
    );
    write(
        root,
        "crates/core",
        "[dependencies]\nbridge={path=\"../bridge\",default-features=false}",
    );
    write(root, "crates/webdock-plugin-protocol", "");
    write(
        root,
        "plugins/allowed",
        "[workspace]\n[dependencies]\nwire={package=\"webdock-plugin-protocol\",path=\"../../crates/webdock-plugin-protocol\"}\n[dev-dependencies]\ncore={path=\"../../crates/core\"}",
    );
    let manifests = architecture::workspace_manifests(root).unwrap();
    for manifest in &manifests {
        lock(manifest);
    }
    let graph = architecture::graph(root, &manifests).unwrap();
    let violations = architecture::violations(&graph);
    assert!(
        violations
            .iter()
            .any(|s| s.ends_with("core -> bridge -> vendor")),
        "{violations:?}"
    );
    assert!(
        violations.iter().any(|s| s.ends_with("worker -> vendor")),
        "{violations:?}"
    );
    assert!(
        !violations.iter().any(|s| s.contains("allowed ->")),
        "dev-only fixture dependencies must not become runtime edges: {violations:?}"
    );
}
#[test]
fn inheritance_and_standalone_policy_drift_fail() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::create_dir(root.join("plugins")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace.lints.rust]\nunused_must_use=\"deny\"\n",
    )
    .unwrap();
    write(root, "crates/core", "");
    write(
        root,
        "plugins/provider",
        "[workspace]\n[lints.rust]\nunused_must_use=\"allow\"",
    );
    let manifests = architecture::workspace_manifests(root).unwrap();
    let violations = architecture::lint_policy(root, &manifests).unwrap();
    assert_eq!(violations.len(), 2);
    write(root, "crates/core", "[lints]\nworkspace=true");
    write(
        root,
        "plugins/provider",
        "[workspace]\n[lints.rust]\nunused_must_use=\"deny\"",
    );
    assert!(
        architecture::lint_policy(root, &manifests)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn export_protocol_and_byte_transport_are_shared_but_cannot_pull_domain_implementations() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers=[\"crates/*\"]\nexclude=[\"plugins/consumer\"]\nresolver=\"3\"\n",
    )
    .unwrap();
    write(root, "crates/core-domain", "");
    write(root, "crates/webdock-export-protocol", "");
    write(root, "crates/webdock-object-storage", "");
    write(
        root,
        "plugins/consumer",
        "[workspace]\n[dependencies]\nwebdock-export-protocol={path=\"../../crates/webdock-export-protocol\"}\nwebdock-object-storage={path=\"../../crates/webdock-object-storage\"}",
    );
    lock(&root.join("Cargo.toml"));
    lock(&root.join("plugins/consumer/Cargo.toml"));
    let manifests = architecture::workspace_manifests(root).unwrap();
    let graph = architecture::graph(root, &manifests).unwrap();
    assert!(architecture::violations(&graph).is_empty());
    write(
        root,
        "crates/webdock-object-storage",
        "[dependencies]\ncore-domain={path=\"../core-domain\"}",
    );
    lock(&root.join("Cargo.toml"));
    lock(&root.join("plugins/consumer/Cargo.toml"));
    let graph = architecture::graph(root, &manifests).unwrap();
    assert!(
        architecture::violations(&graph)
            .iter()
            .any(|finding| finding.contains("webdock-object-storage -> core-domain"))
    );
}
