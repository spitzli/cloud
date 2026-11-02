//! Disposable nonroot container entrypoint; all transfers happen through explicit files.
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
use webdock_build::{
    Error, Result,
    container_runner::{
        BuildSecrets, ContainerBuildRequest, SourceArchive, TrustedTools, read_input_file,
        require_container, run,
    },
    execution::MAX_LOG,
};
fn fail(code: &'static str) -> Error {
    Error {
        code,
        logs: String::new(),
    }
}
fn io(_: std::io::Error) -> Error {
    fail("runner_storage_unavailable")
}
struct Paths {
    request: PathBuf,
    metadata: PathBuf,
    source: PathBuf,
    tools: PathBuf,
    secrets: PathBuf,
    output: PathBuf,
    cancel: PathBuf,
}
impl Paths {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let mut paths = Self {
            request: "/workspace/input/request.json".into(),
            metadata: "/workspace/input/source.json".into(),
            source: "/workspace/input/source.tar".into(),
            tools: "/etc/webdock-build/tools.json".into(),
            secrets: "/workspace/input/secrets.json".into(),
            output: "/workspace/output".into(),
            cancel: "/workspace/cancel".into(),
        };
        let mut seen = std::collections::BTreeSet::new();
        while let Some(key) = args.next() {
            if !seen.insert(key.clone()) {
                return Err(fail("invalid_runner_arguments"));
            }
            let value = PathBuf::from(
                args.next()
                    .ok_or_else(|| fail("invalid_runner_arguments"))?,
            );
            if !value.is_absolute()
                || value.components().any(|c| {
                    !matches!(
                        c,
                        std::path::Component::RootDir | std::path::Component::Normal(_)
                    )
                })
            {
                return Err(fail("invalid_runner_arguments"));
            }
            match key.as_str() {
                "--request" => paths.request = value,
                "--source-metadata" => paths.metadata = value,
                "--source" => paths.source = value,
                "--tools" => paths.tools = value,
                "--secrets" => paths.secrets = value,
                "--output-dir" => paths.output = value,
                "--cancel-file" => paths.cancel = value,
                _ => return Err(fail("invalid_runner_arguments")),
            }
        }
        Ok(paths)
    }
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path, limit: u64) -> Result<T> {
    serde_json::from_slice(&read_input_file(path, limit)?).map_err(|_| fail("invalid_runner_input"))
}
fn create(path: &Path) -> Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(io)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceMetadata {
    source_sha: String,
    archive_sha256: String,
}
fn execute(paths: &Paths) -> Result<()> {
    let request: ContainerBuildRequest = read_json(&paths.request, 64 * 1024)?;
    request.validate()?;
    let metadata: SourceMetadata = read_json(&paths.metadata, 1024)?;
    if std::env::var("WEBDOCK_COMMIT_SHA").is_ok_and(|sha| sha != metadata.source_sha) {
        return Err(fail("source_sha_mismatch"));
    }
    let source =
        SourceArchive::from_file(&paths.source, metadata.source_sha, metadata.archive_sha256)?;
    let tools: TrustedTools = read_json(&paths.tools, 16 * 1024)?;
    let secrets = if paths.secrets.try_exists().map_err(io)? {
        BuildSecrets::new(read_json::<BTreeMap<String, String>>(
            &paths.secrets,
            20 * 1024,
        )?)?
    } else {
        BuildSecrets::default()
    };
    let mut events = create(&paths.output.join("events.jsonl"))?;
    let mut log = create(&paths.output.join("build.log"))?;
    let mut log_bytes = 0usize;
    let result = run(
        &request,
        source,
        &tools,
        secrets,
        &mut |event| {
            if log_bytes + event.logs.len() > MAX_LOG {
                return Err(fail("log_limit"));
            }
            log_bytes += event.logs.len();
            log.write_all(event.logs.as_bytes()).map_err(io)?;
            log.flush().map_err(io)?;
            let mut line = serde_json::to_vec(&event).map_err(|_| fail("invalid_runner_event"))?;
            line.push(b'\n');
            events.write_all(&line).map_err(io)?;
            events.flush().map_err(io)?;
            std::io::stdout().write_all(&line).map_err(io)?;
            std::io::stdout().flush().map_err(io)
        },
        &mut || {
            if paths.cancel.try_exists().map_err(io)? {
                Err(fail("build_cancelled"))
            } else {
                Ok(())
            }
        },
    )?;
    let mut artifact = create(&paths.output.join("image.tar"))?;
    artifact.write_all(&result.artifact).map_err(io)?;
    artifact.sync_all().map_err(io)?;
    let status = json!({"success":true,"sourceSha":request.source_sha,"archiveSha256":request.archive_sha256,"artifactDigest":result.digest,"imageDigest":result.image_digest,"artifactBytes":result.artifact.len()});
    create(&paths.output.join("status.json"))?
        .write_all(status.to_string().as_bytes())
        .map_err(io)
}
#[cfg(feature = "worker")]
fn transfer(mode: &str, mut args: impl Iterator<Item = String>) -> Result<()> {
    use webdock_build::kubernetes_transfer::{
        AttemptTransfer, MAX_HANDOFF_BYTES, TransferClient, TransferPolicy,
    };
    let mut config = PathBuf::from("/handoff/attempt.json");
    let mut policy = PathBuf::from("/etc/webdock-build/transfer-policy.json");
    let mut directory = PathBuf::from(if mode == "download" {
        "/workspace/input"
    } else {
        "/workspace/output"
    });
    let mut seen = std::collections::BTreeSet::new();
    while let Some(key) = args.next() {
        if !seen.insert(key.clone()) {
            return Err(fail("invalid_runner_arguments"));
        }
        let value = PathBuf::from(
            args.next()
                .ok_or_else(|| fail("invalid_runner_arguments"))?,
        );
        if !value.is_absolute()
            || value.components().any(|c| {
                !matches!(
                    c,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
        {
            return Err(fail("invalid_runner_arguments"));
        }
        match key.as_str() {
            "--config" => config = value,
            "--policy" => policy = value,
            "--input-dir" if mode == "download" => directory = value,
            "--output-dir" if mode == "upload" => directory = value,
            _ => return Err(fail("invalid_runner_arguments")),
        }
    }
    let attempt: AttemptTransfer = read_json(&config, MAX_HANDOFF_BYTES)?;
    for (key, expected) in [
        ("WEBDOCK_CUSTOMER_ID", attempt.customer_id.as_str()),
        ("WEBDOCK_PROJECT_ID", attempt.project_id.as_str()),
        ("WEBDOCK_BUILD_ID", attempt.build_id.as_str()),
        ("WEBDOCK_COMMIT_SHA", attempt.request.source_sha.as_str()),
    ] {
        if std::env::var(key).is_ok_and(|v| v != expected) {
            return Err(fail("transfer_identity_mismatch"));
        }
    }
    if std::env::var("WEBDOCK_GENERATION").is_ok_and(|v| v != attempt.generation.to_string()) {
        return Err(fail("transfer_identity_mismatch"));
    }
    let policy: TransferPolicy = read_json(&policy, 16 * 1024)?;
    let client = TransferClient::new(policy)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| fail("transfer_runtime_unavailable"))?;
    if mode == "download" {
        runtime.block_on(client.download(&attempt, &directory))
    } else {
        let uid =
            std::env::var("WEBDOCK_JOB_UID").map_err(|_| fail("invalid_transfer_job_identity"))?;
        runtime.block_on(client.upload(&attempt, &uid, &directory))
    }
}
fn main() -> std::process::ExitCode {
    let result = (|| {
        require_container()?;
        let mut arguments = std::env::args().skip(1);
        let first = arguments.next();
        if first.as_deref() == Some("proxy-url") {
            let path = match arguments.next().as_deref() {
                None => PathBuf::from("/etc/webdock-build/tools.json"),
                Some("--tools") => PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| fail("invalid_runner_arguments"))?,
                ),
                _ => return Err(fail("invalid_runner_arguments")),
            };
            if arguments.next().is_some() {
                return Err(fail("invalid_runner_arguments"));
            }
            let tools: TrustedTools = read_json(&path, 16 * 1024)?;
            tools.validate()?;
            println!("{}", tools.proxy_url.as_deref().unwrap_or(""));
            return Ok(());
        }

        #[cfg(feature = "worker")]
        if matches!(first.as_deref(), Some("download" | "upload")) {
            return transfer(first.as_deref().unwrap(), arguments);
        }
        let paths = Paths::parse(first.into_iter().chain(arguments))?;
        // The output directory is created once, separate from immutable source input.
        std::fs::create_dir(&paths.output).map_err(io)?;
        std::fs::set_permissions(&paths.output, std::fs::Permissions::from_mode(0o700))
            .map_err(io)?;
        let result = execute(&paths);
        if let Err(error) = &result {
            let status = json!({"success":false,"errorCode":error.code});
            create(&paths.output.join("status.json"))?
                .write_all(status.to_string().as_bytes())
                .map_err(io)?;
        }
        result
    })();
    if let Err(error) = result {
        eprintln!("{}", error.code);
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
