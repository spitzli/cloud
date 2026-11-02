//! Explicit command plans; no shell expansion, ambient production configuration or ignored exit.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Contracts,
    Rust,
    Frontend,
    All,
}
impl Scope {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "contracts" => Some(Self::Contracts),
            "rust" => Some(Self::Rust),
            "frontend" => Some(Self::Frontend),
            "all" => Some(Self::All),
            _ => None,
        }
    }
}
pub fn arguments(
    values: &[std::ffi::OsString],
    default_root: PathBuf,
) -> Result<(Scope, PathBuf), String> {
    let (scope, root) = match values {
        [] => (Some(Scope::Contracts), default_root),
        [scope] => (scope.to_str().and_then(Scope::parse), default_root),
        [scope, root] => (scope.to_str().and_then(Scope::parse), PathBuf::from(root)),
        _ => (None, default_root),
    };
    let scope =
        scope.ok_or("Usage: webdock-quality [contracts|rust|frontend|all] [repository-root]")?;
    let root = root
        .canonicalize()
        .map_err(|error| format!("Repository root unavailable: {error}"))?;
    Ok((scope, root))
}
#[derive(Debug, PartialEq, Eq)]
pub struct Check {
    pub program: String,
    pub args: Vec<String>,
}
impl Check {
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|s| (*s).into()).collect(),
        }
    }
}
fn rust_checks(manifests: &[PathBuf]) -> Vec<Check> {
    let mut checks = vec![
        Check::new("cargo", &["fmt", "--all", "--", "--check"]),
        Check::new(
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--locked",
                "--",
                "-D",
                "warnings",
            ],
        ),
        Check::new(
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--no-default-features",
                "--locked",
                "--",
                "-D",
                "warnings",
            ],
        ),
    ];
    // Cargo feature unification can hide an isolated package's empty/partial-feature errors.
    for features in [None, Some("docker"), Some("kubernetes")] {
        let mut check = Check::new(
            "cargo",
            &[
                "clippy",
                "-p",
                "webdock-agent",
                "--all-targets",
                "--no-default-features",
                "--locked",
            ],
        );
        if let Some(features) = features {
            check.args.extend(["--features".into(), features.into()]);
        }
        check
            .args
            .extend(["--".into(), "-D".into(), "warnings".into()]);
        checks.push(check);
    }
    checks.push(Check::new(
        "cargo",
        &[
            "clippy",
            "-p",
            "webdock-build",
            "--all-targets",
            "--no-default-features",
            "--features",
            "worker",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
    ));
    checks.push(Check::new(
        "cargo",
        &["test", "--workspace", "--lib", "--all-features", "--locked"],
    ));
    checks.push(Check::new(
        "cargo",
        &[
            "test",
            "-p",
            "webdock-object-storage",
            "--test",
            "transfers",
            "--features",
            "local-test",
            "--locked",
        ],
    ));
    checks.push(Check::new(
        "cargo",
        &[
            "test",
            "-p",
            "webdock-build",
            "--test",
            "artifact",
            "--test",
            "contracts",
            "--test",
            "execution",
            "--test",
            "remote_artifacts",
            "--no-default-features",
            "--features",
            "worker",
            "--locked",
        ],
    ));
    checks.push(Check::new(
        "python3",
        &[
            "-B",
            "-m",
            "unittest",
            "discover",
            "-s",
            "tools/tests",
            "-p",
            "test_*storage*.py",
        ],
    ));
    checks.push(Check::new(
        "cargo",
        &[
            "test",
            "-p",
            "webdock-agent-protocol",
            "--tests",
            "--locked",
        ],
    ));
    checks.push(Check::new(
        "cargo",
        &[
            "test",
            "-p",
            "webdock-supervisor",
            "--test",
            "contracts",
            "--no-default-features",
            "--features",
            "journal",
            "--locked",
        ],
    ));
    // These suites use local files and controlled RPC peers. No Engine/cluster access is enabled.
    checks.push(Check::new(
        "cargo",
        &[
            "test",
            "-p",
            "webdock-agent",
            "--test",
            "management",
            "--test",
            "outbox",
            "--test",
            "config",
            "--no-default-features",
            "--locked",
        ],
    ));
    // Workspace --lib --all-features already covers native runtime guard/ownership unit tests.
    // Run only the new Kubernetes management tests here; common management stays in the
    // no-default pass above. The runtime target uses loopback API/filesystem fixtures by default.
    checks.push(Check::new(
        "cargo",
        &[
            "test",
            "-p",
            "webdock-agent",
            "--test",
            "management",
            "--no-default-features",
            "--features",
            "kubernetes",
            "--locked",
            "plugin_management::",
        ],
    ));
    checks.push(Check::new(
        "cargo",
        &[
            "test",
            "-p",
            "webdock-agent",
            "--test",
            "plugin_runtime",
            "--no-default-features",
            "--features",
            "kubernetes",
            "--locked",
        ],
    ));
    // Pure manifest builders: include both adapters, without executing either runtime.
    checks.push(Check::new(
        "cargo",
        &[
            "test",
            "-p",
            "webdock-agent",
            "--test",
            "policy",
            "--all-features",
            "--locked",
        ],
    ));
    checks.push(Check::new(
        "cargo",
        &[
            "test",
            "-p",
            "webdock-control-plane",
            "--test",
            "http",
            "--test",
            "ui",
            "--test",
            "offline",
            "--test",
            "resource_api",
            "--locked",
        ],
    ));
    checks.push(Check::new(
        "cargo",
        &[
            "test",
            "-p",
            "webdock-plugin-protocol",
            "--test",
            "contracts",
            "--test",
            "release",
            "--locked",
        ],
    ));
    checks.push(Check::new(
        "cargo",
        &[
            "test",
            "-p",
            "webdock-plugin-runtime",
            "--test",
            "transport",
            "--test",
            "jobs",
            "--test",
            "job_host",
            "--test",
            "runtime_authorization",
            "--locked",
        ],
    ));
    checks.push(Check::new(
        "cargo",
        &[
            "test",
            "-p",
            "webdock-mail-runtime",
            "-p",
            "webdock-mail-protocol",
            "-p",
            "webdock-mail-message",
            "--tests",
            "--all-features",
            "--locked",
        ],
    ));
    checks.push(Check::new(
        "cargo",
        &[
            "test",
            "-p",
            "webdock-plugin-host",
            "--test",
            "mail_outbox_catcher",
            "--test",
            "mail_outbox_native",
            "--locked",
        ],
    ));
    for manifest in manifests {
        let path = manifest.to_string_lossy();
        checks.push(Check::new(
            "cargo",
            &["fmt", "--manifest-path", &path, "--", "--check"],
        ));
        checks.push(Check::new(
            "cargo",
            &[
                "clippy",
                "--manifest-path",
                &path,
                "--all-targets",
                "--all-features",
                "--locked",
                "--",
                "-D",
                "warnings",
            ],
        ));
        checks.push(Check::new(
            "cargo",
            &[
                "test",
                "--manifest-path",
                &path,
                "--all-features",
                "--locked",
            ],
        ));
    }
    checks
}
pub fn plan(scope: Scope, standalone_manifests: &[PathBuf]) -> Vec<Check> {
    let mut checks = Vec::new();
    if matches!(scope, Scope::All | Scope::Rust) {
        checks.extend(rust_checks(standalone_manifests));
    }
    if matches!(scope, Scope::All | Scope::Frontend) {
        checks.push(Check::new(
            "bun",
            &[
                "--no-env-file",
                "test",
                "tools/tests/native-plugin-fixture.test.mjs",
            ],
        ));
        for script in ["lint", "typecheck", "test", "build", "test:browser"] {
            checks.push(Check::new(
                "bun",
                &["run", "--filter", "@webdock/console", script],
            ));
        }
    }
    checks
}
pub fn execute(root: &Path, checks: &[Check]) -> Result<(), String> {
    if checks.iter().any(|check| check.program == "bun") {
        // Bun is the only JavaScript runtime and package manager; Node is not used by this gate.
        let bun = Command::new("bun")
            .arg("--version")
            .output()
            .map_err(|_| "Bun is required for Console checks".to_owned())?;
        if !bun.status.success() {
            return Err("Bun is required for Console checks".into());
        }
    }
    for check in checks {
        println!("Checking: {} {}", check.program, check.args.join(" "));
        let status = Command::new(&check.program)
            .args(&check.args)
            .current_dir(root)
            .status()
            .map_err(|error| format!("Cannot start {}: {error}", check.program))?;
        if !status.success() {
            return Err(format!(
                "Quality check failed: {} {}",
                check.program,
                check.args.join(" ")
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_plan_enforces_features_standalone_plugins_and_all_console_checks() {
        let manifests = [
            PathBuf::from("plugins/provider/Cargo.toml"),
            PathBuf::from("tools/quality/Cargo.toml"),
        ];
        let checks = plan(Scope::All, &manifests);
        assert_eq!(
            checks.iter().filter(|check| check.program == "bun").count(),
            6
        );
        for path in manifests {
            for command in ["fmt", "clippy", "test"] {
                assert!(
                    checks
                        .iter()
                        .any(|check| check.args.first().is_some_and(|s| s == command)
                            && check.args.iter().any(|s| s == path.to_str().unwrap()))
                );
            }
        }
        assert!(checks.iter().any(|check| {
            check.program == "bun"
                && check.args
                    == [
                        "--no-env-file",
                        "test",
                        "tools/tests/native-plugin-fixture.test.mjs",
                    ]
        }));
        let isolated = checks
            .iter()
            .filter(|check| {
                check.args.first().is_some_and(|value| value == "clippy")
                    && check.args.iter().any(|s| s == "webdock-agent")
            })
            .collect::<Vec<_>>();
        assert_eq!(isolated.len(), 3);
        assert!(
            isolated
                .iter()
                .all(|check| check.args.iter().any(|s| s == "--no-default-features"))
        );
        assert!(
            checks
                .iter()
                .filter(|check| matches!(
                    check.args.first().map(String::as_str),
                    Some("clippy" | "test")
                ))
                .all(|check| check.args.iter().any(|s| s == "--locked"))
        );
        assert!(plan(Scope::Contracts, &[]).is_empty());
        assert!(
            plan(Scope::Frontend, &[])
                .iter()
                .all(|check| check.program == "bun")
        );
        for package in [
            "webdock-agent-protocol",
            "webdock-supervisor",
            "webdock-agent",
        ] {
            let check = checks
                .iter()
                .find(|check| {
                    check.args.first().is_some_and(|value| value == "test")
                        && check.args.iter().any(|value| value == package)
                })
                .expect("Protocol, supervisor and isolated daemon regressions must run in CI");
            assert!(!check.args.iter().any(|value| value == "--ignored"));
            if package == "webdock-agent" {
                assert!(
                    check
                        .args
                        .iter()
                        .any(|value| value == "--no-default-features")
                );
                for suite in ["management", "outbox", "config"] {
                    assert!(check.args.iter().any(|value| value == suite));
                }
            }
            if package == "webdock-supervisor" {
                assert!(check.args.iter().any(|value| value == "journal"));
            }
        }
        assert!(checks.iter().any(|check| {
            check.args.first().is_some_and(|value| value == "test")
                && check.args.iter().any(|value| value == "policy")
                && check.args.iter().any(|value| value == "--all-features")
        }));
    }
    #[test]
    fn native_plugin_suites_run_once_with_kubernetes_without_repeating_workspace_libs() {
        fn argument(check: &Check, flag: &str, value: &str) -> bool {
            check
                .args
                .windows(2)
                .any(|pair| pair[0] == flag && pair[1] == value)
        }
        let checks = plan(Scope::Rust, &[]);
        let tests = checks
            .iter()
            .filter(|check| check.args.first().is_some_and(|arg| arg == "test"))
            .collect::<Vec<_>>();
        let agent = tests
            .iter()
            .copied()
            .filter(|check| argument(check, "-p", "webdock-agent"))
            .collect::<Vec<_>>();
        for suite in ["plugin_runtime", "management"] {
            let native = agent
                .iter()
                .copied()
                .filter(|check| {
                    argument(check, "--test", suite) && argument(check, "--features", "kubernetes")
                })
                .collect::<Vec<_>>();
            assert_eq!(native.len(), 1, "one Kubernetes invocation for {suite}");
            assert!(
                native[0]
                    .args
                    .iter()
                    .any(|arg| arg == "--no-default-features")
            );
            assert!(
                !native[0]
                    .args
                    .iter()
                    .any(|arg| arg == "--ignored" || arg == "--include-ignored")
            );
            if suite == "management" {
                assert!(
                    native[0]
                        .args
                        .iter()
                        .any(|arg| arg == "plugin_management::"),
                    "avoid repeating the common management suites"
                );
            }
        }
        assert_eq!(
            tests
                .iter()
                .filter(|check| check.args.iter().any(|arg| arg == "--workspace")
                    && check.args.iter().any(|arg| arg == "--lib"))
                .count(),
            1,
            "native library guards already run in the existing all-features workspace pass"
        );
        assert!(
            agent
                .iter()
                .all(|check| !check.args.iter().any(|arg| arg == "--lib")),
            "do not duplicate agent unit tests"
        );
        assert!(
            tests.iter().any(|check| {
                ["--workspace", "--lib", "--all-features"]
                    .iter()
                    .all(|flag| check.args.iter().any(|arg| arg == flag))
            }),
            "native library tests require Kubernetes through the existing all-features pass"
        );

        let protocol = tests
            .iter()
            .find(|check| argument(check, "-p", "webdock-agent-protocol"))
            .unwrap();
        assert!(
            protocol.args.iter().any(|arg| arg == "--tests"),
            "all agent protocol integration tests include plugin DTO regressions"
        );
        let releases = tests
            .iter()
            .copied()
            .filter(|check| {
                argument(check, "-p", "webdock-plugin-protocol")
                    && argument(check, "--test", "release")
            })
            .collect::<Vec<_>>();
        assert_eq!(releases.len(), 1);
        assert!(argument(releases[0], "--test", "contracts"));
        let runtime = tests
            .iter()
            .find(|check| argument(check, "-p", "webdock-plugin-runtime"))
            .unwrap();
        for suite in ["transport", "jobs", "job_host", "runtime_authorization"] {
            assert!(
                argument(runtime, "--test", suite),
                "missing native runtime suite {suite}"
            );
        }
        assert!(
            tests
                .iter()
                .all(|check| check.args.iter().any(|arg| arg == "--locked"))
        );
    }
    #[test]
    fn mail_sending_protocol_runtime_and_catcher_run_without_infrastructure_opt_ins() {
        let checks = plan(Scope::Rust, &[]);
        let runtime = checks
            .iter()
            .find(|check| {
                check.args.iter().any(|arg| arg == "webdock-mail-runtime")
                    && check.args.first().is_some_and(|arg| arg == "test")
            })
            .expect("Mail execution integration tests must run in quality");
        for required in [
            "webdock-mail-protocol",
            "webdock-mail-message",
            "--tests",
            "--all-features",
            "--locked",
        ] {
            assert!(runtime.args.iter().any(|arg| arg == required));
        }
        let catcher = checks
            .iter()
            .find(|check| check.args.iter().any(|arg| arg == "mail_outbox_catcher"))
            .expect("The bounded SMTP fault helper must run in quality");
        assert!(catcher.args.iter().any(|arg| arg == "webdock-plugin-host"));
        assert!(
            catcher
                .args
                .windows(2)
                .any(|pair| pair == ["--test", "mail_outbox_native"]),
            "The safe native outbox custody regressions must run in quality"
        );
        assert!(!checks.iter().any(|check| {
            check
                .args
                .iter()
                .any(|arg| arg == "--include-ignored" || arg == "--ignored")
        }));
    }
    #[test]
    fn s3_and_artifact_regressions_run_without_live_storage() {
        let checks = plan(Scope::Rust, &[]);
        for (package, suite) in [
            ("webdock-object-storage", "transfers"),
            ("webdock-build", "remote_artifacts"),
        ] {
            assert!(
                checks.iter().any(|check| {
                    check.args.first().is_some_and(|arg| arg == "test")
                        && check.args.iter().any(|arg| arg == package)
                        && check.args.windows(2).any(|pair| pair == ["--test", suite])
                        && !check.args.iter().any(|arg| arg == "--include-ignored")
                }),
                "missing offline {package}/{suite} regressions"
            );
        }
    }
    #[test]
    fn unknown_scope_is_not_silently_replaced() {
        assert_eq!(Scope::parse("all"), Some(Scope::All));
        assert_eq!(Scope::parse("skip-errors"), None);
    }
    #[test]
    fn relative_root_is_resolved_before_manifest_paths_and_working_directory() {
        let (scope, root) =
            arguments(&["rust".into(), ".".into()], PathBuf::from("unused")).unwrap();
        assert_eq!(scope, Scope::Rust);
        assert!(root.is_absolute());
        assert_eq!(
            root,
            std::env::current_dir().unwrap().canonicalize().unwrap()
        );
        assert!(arguments(&["skip-errors".into()], PathBuf::from(".")).is_err());
    }
}
