use std::{collections::BTreeMap, os::unix::fs::symlink, path::Path, process::Command};
use webdock_build::{
    artifact,
    frontend::{self, FrontendConfig, configure_exporters},
};

fn config(directory: &str) -> FrontendConfig {
    FrontendConfig {
        version: 1,
        directory: directory.into(),
    }
}

#[test]
fn bounded_configuration_accepts_only_versioned_relative_directory() {
    assert_eq!(
        FrontendConfig::parse(br#"{"version":1,"directory":"app/dist"}"#).unwrap(),
        config("app/dist")
    );
    for bytes in [
        br#"{"version":2,"directory":"dist"}"#.as_slice(),
        br#"{"directory":"dist"}"#,
        br#"{"version":1,"directory":"dist","exporter":"local"}"#,
        br#"{"version":1,"directory":"dist","directory":"other"}"#,
    ] {
        assert_eq!(
            FrontendConfig::parse(bytes).unwrap_err().code,
            "invalid_frontend_config"
        );
    }
    let oversized = format!(
        "{}{{\"version\":1,\"directory\":\"dist\"}}",
        " ".repeat(frontend::MAX_CONFIG)
    );
    assert_eq!(
        FrontendConfig::parse(oversized.as_bytes())
            .unwrap_err()
            .code,
        "frontend_config_limit"
    );
    assert_eq!(frontend::CONFIG_PATH, ".webdock/frontend.json");
    for directory in [
        "",
        ".",
        "..",
        "/dist",
        "a/../dist",
        "a/./dist",
        "a//dist",
        "a\\dist",
        "a/",
        "a\0b",
    ] {
        let bytes = serde_json::to_vec(&config(directory)).unwrap();
        assert!(
            FrontendConfig::parse(&bytes).is_err(),
            "accepted {directory:?}"
        );
    }
}

#[test]
fn local_export_is_explicit_and_old_recipe_keeps_only_oci() {
    for (recipe, expected) in [
        (
            "dockerfile",
            vec!["--output", "type=oci,dest=/private/image.tar"],
        ),
        (
            frontend::RECIPE,
            vec![
                "--output",
                "type=oci,dest=/private/image.tar",
                "--output",
                "type=local,dest=/private/rootfs",
            ],
        ),
    ] {
        let mut cmd = Command::new("buildctl");
        configure_exporters(
            &mut cmd,
            recipe,
            Path::new("/private/image.tar"),
            Path::new("/private/rootfs"),
        )
        .unwrap();
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), expected);
    }
    let mut cmd = Command::new("buildctl");
    assert!(
        configure_exporters(
            &mut cmd,
            "vercel",
            Path::new("/image"),
            Path::new("/rootfs")
        )
        .is_err()
    );
    assert_eq!(cmd.get_args().count(), 0);
    assert!(
        configure_exporters(
            &mut cmd,
            frontend::RECIPE,
            Path::new("/a,type=local"),
            Path::new("/rootfs")
        )
        .is_err()
    );
    assert_eq!(cmd.get_args().count(), 0);
}

#[test]
fn retains_exact_selected_regular_files_with_deterministic_bytes() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("app/dist/assets")).unwrap();
    std::fs::write(root.path().join("app/dist/index.html"), "<p>retained</p>").unwrap();
    std::fs::write(
        root.path().join("app/dist/assets/main.js"),
        b"console.log(1)",
    )
    .unwrap();
    std::fs::write(root.path().join("unselected-secret"), "excluded").unwrap();
    let output = config("app/dist")
        .pack(root.path(), artifact::MAX_OUTPUT)
        .unwrap();
    let files = artifact::Archive::read(&output, artifact::MAX_OUTPUT)
        .unwrap()
        .into_files();
    assert_eq!(
        files,
        BTreeMap::from([
            ("index.html".into(), b"<p>retained</p>".to_vec()),
            ("assets/main.js".into(), b"console.log(1)".to_vec()),
        ])
    );
    assert_eq!(
        output,
        config("app/dist")
            .pack(root.path(), artifact::MAX_OUTPUT)
            .unwrap()
    );
    assert!(
        config("missing")
            .pack(root.path(), artifact::MAX_OUTPUT)
            .is_err()
    );
    assert_eq!(
        config("unselected-secret")
            .pack(root.path(), artifact::MAX_OUTPUT)
            .unwrap_err()
            .code,
        "invalid_frontend_output"
    );
}

#[test]
fn selected_tree_rejects_links_and_special_files_instead_of_following_them() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("dist")).unwrap();
    std::fs::write(root.path().join("secret"), "never retain").unwrap();
    symlink("../secret", root.path().join("dist/link")).unwrap();
    assert_eq!(
        config("dist")
            .pack(root.path(), artifact::MAX_OUTPUT)
            .unwrap_err()
            .code,
        "invalid_frontend_output"
    );
    std::fs::remove_file(root.path().join("dist/link")).unwrap();
    std::fs::hard_link(root.path().join("secret"), root.path().join("dist/hard")).unwrap();
    assert!(
        config("dist")
            .pack(root.path(), artifact::MAX_OUTPUT)
            .is_err()
    );
    std::fs::remove_file(root.path().join("dist/hard")).unwrap();
    let socket = std::os::unix::net::UnixListener::bind(root.path().join("dist/socket")).unwrap();
    assert!(
        config("dist")
            .pack(root.path(), artifact::MAX_OUTPUT)
            .is_err()
    );
    drop(socket);
    std::fs::remove_file(root.path().join("dist/socket")).unwrap();
    symlink("dist", root.path().join("alias")).unwrap();
    assert!(
        config("alias")
            .pack(root.path(), artifact::MAX_OUTPUT)
            .is_err()
    );
    symlink(root.path(), root.path().join("outside")).unwrap();
    assert!(
        config("outside/dist")
            .pack(root.path(), artifact::MAX_OUTPUT)
            .is_err()
    );
    assert!(
        config("dist")
            .pack(&root.path().join("outside"), artifact::MAX_OUTPUT)
            .is_err()
    );
}

#[test]
fn bounded_output_counts_tar_overhead_and_empty_directories() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("dist")).unwrap();
    assert_eq!(
        config("dist")
            .pack(root.path(), artifact::MAX_OUTPUT)
            .unwrap_err()
            .code,
        "frontend_output_empty"
    );
    std::fs::write(root.path().join("dist/index.html"), b"abc").unwrap();
    assert_eq!(
        config("dist").pack(root.path(), 3).unwrap_err().code,
        "frontend_output_limit"
    );
    let output = config("dist")
        .pack(root.path(), artifact::MAX_OUTPUT)
        .unwrap();
    assert!(
        config("dist")
            .pack(root.path(), output.len() as u64)
            .is_ok()
    );
    assert_eq!(
        config("dist")
            .pack(root.path(), output.len() as u64 - 1)
            .unwrap_err()
            .code,
        "frontend_output_limit"
    );
    for i in 0..frontend::MAX_ENTRIES {
        std::fs::create_dir(root.path().join(format!("dist/empty-{i}"))).unwrap();
    }
    assert_eq!(
        config("dist")
            .pack(root.path(), artifact::MAX_OUTPUT)
            .unwrap_err()
            .code,
        "frontend_entry_limit"
    );
}
