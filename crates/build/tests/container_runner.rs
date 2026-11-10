#![cfg(feature = "kubernetes-job")]

use sha2::{Digest, Sha256};
use spitzli_build::{
    artifact::Archive,
    container_runner::{
        BuildSecrets, ContainerBuildRequest, PinnedTool, PreparedBuild, RailpackTool,
        SourceArchive, Strategy, TrustedTools,
    },
    execution::Limits,
};
use std::{collections::BTreeMap, path::Path};

fn request() -> ContainerBuildRequest {
    ContainerBuildRequest {
        source_sha: "a".repeat(40),
        archive_sha256: format!("sha256:{}", "0".repeat(64)),
        root_directory: ".".into(),
        context_directory: ".".into(),
        dockerfile: None,
        strategy: Strategy::Auto,
        build_args: BTreeMap::new(),
        limits: Limits {
            cpu: 1,
            memory_mi_b: 512,
            disk_bytes: 10_000_000,
            duration_seconds: 30,
        },
    }
}
fn source(req: &mut ContainerBuildRequest, files: &[(&str, &[u8])]) -> SourceArchive {
    let bytes = Archive::from_files(
        files
            .iter()
            .map(|(k, v)| ((*k).into(), v.to_vec()))
            .collect(),
    )
    .unwrap()
    .encode()
    .unwrap();
    req.archive_sha256 = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
    SourceArchive::from_bytes(req.source_sha.clone(), req.archive_sha256.clone(), bytes).unwrap()
}
fn tools() -> TrustedTools {
    TrustedTools {
        buildctl: PinnedTool {
            path: "/trusted/buildctl".into(),
            sha256: "1".repeat(64),
        },
        railpack: RailpackTool {
            cli: PinnedTool {
                path: "/trusted/railpack".into(),
                sha256: "2".repeat(64),
            },
            version: "0.40.1".into(),
            builder_image: format!(
                "ghcr.io/railwayapp/railpack-builder:mise-2026.9.15@sha256:{}",
                "a".repeat(64)
            ),
            runtime_image: format!(
                "ghcr.io/railwayapp/railpack-runtime:mise-2026.9.15@sha256:{}",
                "b".repeat(64)
            ),
            frontend: format!(
                "ghcr.io/railwayapp/railpack-frontend:v0.40.1@sha256:{}",
                "3".repeat(64)
            ),
        },
        buildkit_socket: "/run/buildkit/buildkitd.sock".into(),
        proxy_url: None,
    }
}
#[test]
fn override_beats_dockerfile_and_auto_prefers_dockerfile() {
    for (override_strategy, expected) in [
        (Strategy::Auto, Strategy::Dockerfile),
        (Strategy::Railpack, Strategy::Railpack),
        (Strategy::Dockerfile, Strategy::Dockerfile),
    ] {
        let mut req = request();
        req.strategy = override_strategy;
        let src = source(&mut req, &[("Dockerfile", b"FROM scratch\n")]);
        let prepared = PreparedBuild::prepare(&req, src).unwrap();
        assert_eq!(prepared.strategy(), expected);
    }
    let mut req = request();
    let src = source(&mut req, &[("package.json", b"{}")]);
    assert_eq!(
        PreparedBuild::prepare(&req, src).unwrap().strategy(),
        Strategy::Railpack
    );
}
#[test]
fn rejects_source_mismatch_and_hostile_selected_paths_before_extracting() {
    let mut req = request();
    let src = source(&mut req, &[("Dockerfile", b"FROM scratch\n")]);
    req.source_sha = "b".repeat(40);
    assert_eq!(
        PreparedBuild::prepare(&req, src).err().unwrap().code,
        "source_sha_mismatch"
    );
    for path in ["../outside", "/etc", "a/../b", "a\\b", "a//b", "./app"] {
        for selector in [0, 1, 2] {
            let mut req = request();
            let src = source(&mut req, &[("Dockerfile", b"FROM scratch\n")]);
            match selector {
                0 => req.root_directory = path.into(),
                1 => req.context_directory = path.into(),
                _ => req.dockerfile = Some(path.into()),
            }
            assert!(
                PreparedBuild::prepare(&req, src).is_err(),
                "selector={selector}, path={path}"
            );
        }
    }
    assert!(
        SourceArchive::from_bytes("a".repeat(40), format!("sha256:{}", "0".repeat(64)), vec![])
            .is_err()
    );
}
#[test]
fn explicit_dockerfile_and_context_are_relative_to_selected_root() {
    let mut req = request();
    req.root_directory = "apps/web".into();
    req.context_directory = "src".into();
    req.dockerfile = Some("deploy/Containerfile".into());
    let src = source(
        &mut req,
        &[
            ("apps/web/src/index.js", b""),
            ("apps/web/deploy/Containerfile", b"FROM scratch\n"),
        ],
    );
    let prepared = PreparedBuild::prepare(&req, src).unwrap();
    assert_eq!(prepared.strategy(), Strategy::Dockerfile);
    let commands = prepared
        .commands(&req, &tools(), &BuildSecrets::default())
        .unwrap();
    let args: Vec<_> = commands[0]
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    assert!(args.iter().any(|a| a.ends_with("/source/apps/web/src")));
    assert!(args.iter().any(|a| a == "filename=Containerfile"));
    assert!(args.iter().any(|a| a.ends_with("/source/apps/web/deploy")));
}
#[test]
fn rejects_runtime_controls_and_mutable_or_mismatched_frontend() {
    for name in [
        "PATH",
        "HOME",
        "LD_LIBRARY_PATH",
        "LD_PRELOAD",
        "BASH_ENV",
        "BUILDKIT_HOST",
        "DOCKER_CONFIG",
        "SPITZLI_WORKER_TOKEN",
        "WEBDOCK_WORKER_TOKEN",
        "GIT_CONFIG_COUNT",
        "PYTHONPATH",
        "RAILPACK_CONFIG_FILE",
        "BUILDKIT_SYNTAX",
    ] {
        let mut req = request();
        req.build_args.insert(name.into(), "x".into());
        assert!(req.validate().is_err(), "{name}");
    }
    let mut t = tools();
    t.railpack.frontend = "ghcr.io/railwayapp/railpack-frontend:latest".into();
    assert!(t.validate().is_err());
    let mut t = tools();
    t.railpack.version = "0.39.0".into();
    assert!(t.validate().is_err());
}
#[test]
fn railpack_prepare_uses_matching_gateway_and_never_puts_secret_values_in_argv() {
    let mut req = request();
    req.build_args
        .insert("PUBLIC_FLAG".into(), "operator-public-env-sentinel".into());
    let src = source(&mut req, &[("package.json", b"{}")]);
    let prepared = PreparedBuild::prepare(&req, src).unwrap();
    let secrets = BuildSecrets::new(BTreeMap::from([(
        "PRIVATE_TOKEN".into(),
        "customer-secret-value".into(),
    )]))
    .unwrap();
    let commands = prepared.commands(&req, &tools(), &secrets).unwrap();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].get_program(), Path::new("/trusted/railpack"));
    let args: Vec<_> = commands[0]
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[0], "prepare");
    assert!(args.contains(&"--plan-out".into()));
    assert!(args.contains(&"--info-out".into()));
    assert!(args.contains(&"PRIVATE_TOKEN".into()));
    assert!(args.contains(&"PUBLIC_FLAG".into()));
    assert!(!args.iter().any(|arg| {
        arg.contains("customer-secret-value") || arg.contains("operator-public-env-sentinel")
    }));
    let prepare_env: BTreeMap<_, _> = commands[0]
        .get_envs()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert_eq!(
        prepare_env["PRIVATE_TOKEN"].as_deref(),
        Some("customer-secret-value")
    );
    assert_eq!(
        prepare_env["PUBLIC_FLAG"].as_deref(),
        Some("operator-public-env-sentinel")
    );
    let build: Vec<_> = commands[1]
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    assert!(build.contains(&"gateway.v0".into()));
    assert!(
        build
            .iter()
            .any(|s| s.starts_with("source=ghcr.io/railwayapp/railpack-frontend:v0.40.1@sha256:"))
    );
    assert!(build.iter().any(|s| s.starts_with("type=oci,dest=")));
    assert!(build.iter().any(|s| s.starts_with("id=PRIVATE_TOKEN,src=")));
    assert!(build.contains(&"id=PUBLIC_FLAG,env=PUBLIC_FLAG".into()));
    let build_env: BTreeMap<_, _> = commands[1]
        .get_envs()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert_eq!(
        build_env["PUBLIC_FLAG"].as_deref(),
        Some("operator-public-env-sentinel")
    );
    assert!(!build_env.contains_key("PRIVATE_TOKEN"));
    for cmd in &commands {
        assert!(
            cmd.get_args()
                .all(|s| !s.to_string_lossy().contains("customer-secret-value"))
        );
        assert!(!cmd.get_args().any(|s| s == "--allow"));
    }
}

#[test]
fn source_file_rejects_symlinks_and_tampering_and_missing_dockerfile() {
    use std::os::unix::fs::symlink;
    let mut req = request();
    let src = source(&mut req, &[("Dockerfile", b"FROM scratch\n")]);
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("source.tar");
    let bytes = Archive::from_files(BTreeMap::from([(
        "Dockerfile".into(),
        b"FROM scratch\n".to_vec(),
    )]))
    .unwrap()
    .encode()
    .unwrap();
    std::fs::write(&input, &bytes).unwrap();
    assert!(
        SourceArchive::from_file(&input, req.source_sha.clone(), req.archive_sha256.clone())
            .is_ok()
    );
    let link = dir.path().join("link.tar");
    symlink(&input, &link).unwrap();
    assert!(
        SourceArchive::from_file(&link, req.source_sha.clone(), req.archive_sha256.clone())
            .is_err()
    );
    let alias = dir.path().join("alias");
    symlink(dir.path(), &alias).unwrap();
    assert!(
        SourceArchive::from_file(
            &alias.join("source.tar"),
            req.source_sha.clone(),
            req.archive_sha256.clone()
        )
        .is_err()
    );
    let prepared = PreparedBuild::prepare(&req, src).unwrap();
    req.build_args.insert("ANOTHER".into(), "changed".into());
    assert_eq!(
        prepared
            .commands(&req, &tools(), &BuildSecrets::default())
            .err()
            .unwrap()
            .code,
        "build_request_changed"
    );
    let mut req = request();
    req.strategy = Strategy::Dockerfile;
    let src = source(&mut req, &[("package.json", b"{}")]);
    assert!(PreparedBuild::prepare(&req, src).is_err());
}

#[test]
fn archive_symlink_is_rejected_before_source_strategy() {
    let mut tar = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_ustar();
    h.set_entry_type(tar::EntryType::Symlink);
    h.set_size(0);
    h.set_mode(0o777);
    h.set_link_name("/etc/passwd").unwrap();
    h.set_cksum();
    tar.append_data(&mut h, "Dockerfile", &b""[..]).unwrap();
    let bytes = tar.into_inner().unwrap();
    let mut req = request();
    req.archive_sha256 = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
    let source =
        SourceArchive::from_bytes(req.source_sha.clone(), req.archive_sha256.clone(), bytes)
            .unwrap();
    assert_eq!(
        PreparedBuild::prepare(&req, source).err().unwrap().code,
        "archive_entry_type"
    );
}

#[test]
#[ignore = "Run only in a nonroot disposable container; invokes trusted fixture scripts, never source code"]
fn container_fake_tools_export_valid_oci_and_redact_customer_secrets() {
    use serde_json::json;
    use spitzli_build::container_runner::{Phase, run};
    use std::os::unix::fs::PermissionsExt;
    let fixture = tempfile::tempdir().unwrap();
    let config = b"{}";
    let config_hash = hex::encode(Sha256::digest(config));
    let manifest=serde_json::to_vec(&json!({"schemaVersion":2,"config":{"mediaType":"application/vnd.oci.image.config.v1+json","size":2,"digest":format!("sha256:{config_hash}")},"layers":[]})).unwrap();
    let hash = hex::encode(Sha256::digest(&manifest));
    let image=Archive::from_files(BTreeMap::from([("oci-layout".into(),br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec()),("index.json".into(),serde_json::to_vec(&json!({"schemaVersion":2,"manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","size":manifest.len(),"digest":format!("sha256:{hash}")}]})).unwrap()),(format!("blobs/sha256/{hash}"),manifest),(format!("blobs/sha256/{config_hash}"),config.to_vec())])).unwrap().encode().unwrap();
    let image_path = fixture.path().join("fixture.tar");
    std::fs::write(&image_path, &image).unwrap();
    fn tool(dir: &Path, name: &str, script: &str) -> PinnedTool {
        let path = dir.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        PinnedTool {
            path,
            sha256: hex::encode(Sha256::digest(script.as_bytes())),
        }
    }
    let fake_build = tool(
        fixture.path(),
        "buildctl",
        &format!(
            "#!/bin/sh\nset -eu\nfor arg in \"$@\"; do case \"$arg\" in type=oci,dest=*) dest=${{arg#type=oci,dest=}};; esac; done\ncp '{}' \"$dest\"\nprintf 'export finished\\n'\n",
            image_path.display()
        ),
    );
    let fake_prepare = tool(
        fixture.path(),
        "railpack",
        "#!/bin/sh\nset -eu\n[ \"$1\" = prepare ]\nshift\nwhile [ $# -gt 0 ]; do case \"$1\" in --plan-out) shift; printf '{}' > \"$1\";; --info-out) shift; printf '{\"success\":true,\"railpackVersion\":\"0.40.1\"}' > \"$1\";; esac; shift; done\nprintf 'build value %s\\n' \"$PRIVATE_TOKEN\"\n",
    );
    let mut tools = tools();
    tools.buildctl = fake_build;
    tools.railpack.cli = fake_prepare;
    let mut req = request();
    let src = source(&mut req, &[("package.json", b"{}")]);
    let secrets = BuildSecrets::new(BTreeMap::from([(
        "PRIVATE_TOKEN".into(),
        "secret-known-only-to-customer".into(),
    )]))
    .unwrap();
    let mut events = Vec::new();
    let result = run(
        &req,
        src,
        &tools,
        secrets,
        &mut |event| {
            events.push(event);
            Ok(())
        },
        &mut || Ok(()),
    )
    .unwrap();
    assert_eq!(result.artifact, image);
    assert_eq!(result.image_digest, format!("sha256:{hash}"));
    assert!(result.logs.contains("[REDACTED]"));
    assert!(!result.logs.contains("secret-known-only-to-customer"));
    assert!(events.iter().any(|e| matches!(e.phase, Phase::Prepare)));
    assert!(events.iter().any(|e| matches!(e.phase, Phase::Complete)));
    assert!(
        events
            .iter()
            .all(|e| !e.logs.contains("secret-known-only-to-customer"))
    );
    let input = fixture.path().join("input");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(
        input.join("request.json"),
        serde_json::to_vec(&req).unwrap(),
    )
    .unwrap();
    std::fs::write(
        input.join("source.json"),
        json!({"sourceSha":req.source_sha,"archiveSha256":req.archive_sha256}).to_string(),
    )
    .unwrap();
    let bytes = Archive::from_files(BTreeMap::from([("package.json".into(), b"{}".to_vec())]))
        .unwrap()
        .encode()
        .unwrap();
    std::fs::write(input.join("source.tar"), bytes).unwrap();
    std::fs::write(
        input.join("tools.json"),
        serde_json::to_vec(&tools).unwrap(),
    )
    .unwrap();
    std::fs::write(
        input.join("secrets.json"),
        json!({"PRIVATE_TOKEN":"secret-known-only-to-customer"}).to_string(),
    )
    .unwrap();
    let output = fixture.path().join("output");
    let binary = std::env::var_os("SPITZLI_TEST_RUNNER_BINARY")
        .unwrap_or_else(|| "/usr/local/bin/spitzli-build-runner".into());
    let invoke = |output: &Path| {
        std::process::Command::new(&binary)
            .arg("--request")
            .arg(input.join("request.json"))
            .arg("--source-metadata")
            .arg(input.join("source.json"))
            .arg("--source")
            .arg(input.join("source.tar"))
            .arg("--tools")
            .arg(input.join("tools.json"))
            .arg("--secrets")
            .arg(input.join("secrets.json"))
            .arg("--output-dir")
            .arg(output)
            .arg("--cancel-file")
            .arg(fixture.path().join("cancel"))
            .output()
            .unwrap()
    };
    let result = invoke(&output);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let status: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output.join("status.json")).unwrap()).unwrap();
    assert_eq!(status["success"], true);
    assert_eq!(status["sourceSha"], req.source_sha);
    assert_eq!(status["imageDigest"], format!("sha256:{hash}"));
    assert_eq!(std::fs::read(output.join("image.tar")).unwrap(), image);
    assert!(!String::from_utf8_lossy(&result.stdout).contains("secret-known-only-to-customer"));
    tools.railpack.cli = tool(
        fixture.path(),
        "bad-railpack",
        "#!/bin/sh\nset -eu\nshift\nwhile [ $# -gt 0 ]; do case \"$1\" in --plan-out) shift; printf '{}' > \"$1\";; --info-out) shift; printf '{\"success\":true,\"railpackVersion\":\"0.39.0\"}' > \"$1\";; esac; shift; done\n",
    );
    let src = source(&mut req, &[("package.json", b"{}")]);
    assert_eq!(
        run(
            &req,
            src,
            &tools,
            BuildSecrets::default(),
            &mut |_| Ok(()),
            &mut || Ok(())
        )
        .err()
        .unwrap()
        .code,
        "railpack_version_mismatch"
    );
    std::fs::write(
        input.join("tools.json"),
        serde_json::to_vec(&tools).unwrap(),
    )
    .unwrap();
    let failure_output = fixture.path().join("failed-output");
    let result = invoke(&failure_output);
    assert!(!result.status.success());
    let status: serde_json::Value =
        serde_json::from_slice(&std::fs::read(failure_output.join("status.json")).unwrap())
            .unwrap();
    assert_eq!(status["success"], false);
    assert_eq!(status["errorCode"], "railpack_version_mismatch");
    assert!(!failure_output.join("image.tar").exists());
}

#[test]
fn live_log_observer_failure_kills_command_group_before_delayed_side_effect() {
    use spitzli_build::{Error, execution::run_bounded_observed};
    let work = tempfile::tempdir().unwrap();
    let marker = work.path().join("must-not-exist");
    let mut command = std::process::Command::new("/bin/sh");
    command
        .args([
            "-c",
            "printf first; sleep 0.3; touch \"$1\"; sleep 10",
            "trusted-log-fixture",
        ])
        .arg(&marker);
    let began = std::time::Instant::now();
    let error = run_bounded_observed(
        &mut command,
        std::time::Duration::from_secs(2),
        1024,
        &mut || Ok(()),
        &mut |_bytes| {
            Err(Error {
                code: "observer_stopped",
                logs: String::new(),
            })
        },
    )
    .err()
    .unwrap();
    assert_eq!(error.code, "observer_stopped");
    assert!(began.elapsed() < std::time::Duration::from_secs(1));
    std::thread::sleep(std::time::Duration::from_millis(400));
    assert!(!marker.exists());
}

#[test]
fn live_log_observer_never_receives_bytes_beyond_existing_log_cap() {
    let mut command = std::process::Command::new("/usr/bin/head");
    command.args(["-c", "25000", "/dev/zero"]);
    let mut observed = 0usize;
    let error = spitzli_build::execution::run_bounded_observed(
        &mut command,
        std::time::Duration::from_secs(2),
        1024,
        &mut || Ok(()),
        &mut |bytes| {
            observed += bytes.len();
            Ok(())
        },
    )
    .err()
    .unwrap();
    assert_eq!(error.code, "log_limit");
    assert!(observed > 0);
    assert!(observed <= spitzli_build::execution::MAX_LOG);
}

#[test]
fn operator_proxy_is_fixed_and_cannot_be_controlled_by_customer_values() {
    for key in [
        "HTTP_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "no_proxy",
        "FTP_PROXY",
    ] {
        let mut request = request();
        request
            .build_args
            .insert(key.into(), "http://evil.example".into());
        assert!(request.validate().is_err());
    }
    let mut tools = tools();
    tools.proxy_url = Some("http://10.83.0.1:3128".into());
    tools.validate().unwrap();
    let mut request = request();
    let src = source(&mut request, &[("Dockerfile", b"FROM scratch\n")]);
    let prepared = PreparedBuild::prepare(&request, src).unwrap();
    let commands = prepared
        .commands(&request, &tools, &BuildSecrets::default())
        .unwrap();
    let args: Vec<_> = commands[0]
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    assert!(args.contains(&"build-arg:HTTPS_PROXY=http://10.83.0.1:3128".into()));
    assert!(
        commands[0]
            .get_envs()
            .any(|(key, value)| key == "HTTPS_PROXY"
                && value.is_some_and(|v| v == "http://10.83.0.1:3128"))
    );
    tools.proxy_url = Some("http://user:password@10.83.0.1:3128".into());
    assert!(tools.validate().is_err());
}

#[test]
fn git_file_modes_survive_container_context_without_exposing_private_workspace() {
    use std::os::unix::fs::PermissionsExt;
    let mut tar = tar::Builder::new(Vec::new());
    for (name, mode) in [
        ("bin/start", 0o755),
        ("config.txt", 0o644),
        ("Dockerfile", 0o644),
    ] {
        let bytes = if name == "Dockerfile" {
            b"FROM scratch\n".as_slice()
        } else {
            b"owned".as_slice()
        };
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        tar.append_data(&mut header, name, bytes).unwrap();
    }
    let bytes = tar.into_inner().unwrap();
    let private = tempfile::tempdir().unwrap();
    std::fs::set_permissions(private.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    Archive::read(&bytes, 1024 * 1024)
        .unwrap()
        .extract_private(private.path())
        .unwrap();
    assert_eq!(
        std::fs::metadata(private.path().join("bin/start"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(private.path().join("config.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let mut req = request();
    req.archive_sha256 = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
    let src = SourceArchive::from_bytes(req.source_sha.clone(), req.archive_sha256.clone(), bytes)
        .unwrap();
    let prepared = PreparedBuild::prepare(&req, src).unwrap();
    let commands = prepared
        .commands(&req, &tools(), &BuildSecrets::default())
        .unwrap();
    let context = commands[0]
        .get_args()
        .filter_map(|s| s.to_str())
        .find_map(|s| s.strip_prefix("context="))
        .unwrap();
    let context = Path::new(context);
    assert_eq!(
        std::fs::metadata(context.join("bin/start"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755,
        "Docker COPY must preserve Git executability for nonroot runtime users"
    );
    assert_eq!(
        std::fs::metadata(context.join("config.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    assert_eq!(
        std::fs::metadata(context.join("bin"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        std::fs::metadata(context.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700,
        "the enclosing attempt workspace remains private"
    );
}

#[test]
fn source_dockerfile_cannot_choose_a_custom_frontend_with_directive_variants() {
    for dockerfile in [
        "#syntax=evil/frontend:v1\nFROM scratch\n",
        "\u{feff} \t#  SyNtAx \t= evil/frontend\nFROM scratch\n",
        "FROM scratch\n# CHECK=skip=all\n",
        "#\u{feff}syntax=evil/frontend\nFROM scratch\n",
    ] {
        let mut request = request();
        let src = source(&mut request, &[("Dockerfile", dockerfile.as_bytes())]);
        assert_eq!(
            PreparedBuild::prepare(&request, src).err().unwrap().code,
            "unsupported_dockerfile_directive"
        );
    }
}
