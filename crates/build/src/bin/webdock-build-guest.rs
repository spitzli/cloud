//! This binary belongs ONLY in an administrator-prepared disposable VM image.
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
    process::Command,
    time::Duration,
};
use webdock_build::{
    artifact::{Archive, MAX_OUTPUT, MAX_SOURCE},
    execution::{GuestRequest, run_bounded},
};
fn build() -> webdock_build::Result<Vec<u8>> {
    let mut input = Vec::new();
    std::fs::File::open("/dev/vdb")
        .unwrap()
        .take(MAX_OUTPUT + 1)
        .read_to_end(&mut input)
        .unwrap();
    let archive = Archive::read(&input, MAX_SOURCE)?;
    let work = tempfile::tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(work.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    archive.extract_private(work.path())?;
    let job: GuestRequest = serde_json::from_slice(&archive.files()["job.json"]).unwrap();
    job.validate()?;
    let root = work.path().join("source").join(&job.root_directory);
    let frontend = if job.recipe == webdock_build::frontend::RECIPE {
        Some(
            webdock_build::frontend::FrontendConfig::from_source(
                &archive,
                &format!(
                    "source{}",
                    if job.root_directory == "." {
                        String::new()
                    } else {
                        format!("/{}", job.root_directory)
                    }
                ),
            )?
            .0,
        )
    } else {
        None
    };
    let output = work.path().join("image.tar");
    let rootfs = work.path().join("rootfs");
    let mut command = Command::new("/usr/bin/buildctl");
    command
        .args(["build", "--frontend", "dockerfile.v0", "--local"])
        .arg(format!("context={}", root.display()))
        .arg("--local")
        .arg(format!("dockerfile={}", root.display()));
    webdock_build::frontend::configure_exporters(&mut command, &job.recipe, &output, &rootfs)?;
    for (k, v) in &job.build_environment {
        command.arg("--opt").arg(format!("build-arg:{k}={v}"));
    }
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/tmp/build-home")
        .env("CI", "1")
        .current_dir(root);
    let result = run_bounded(
        &mut command,
        Duration::from_secs(job.limits.duration_seconds.saturating_sub(10)),
        job.limits.disk_bytes,
        &mut || Ok(()),
    );
    let (success, logs) = match result {
        Ok(logs) => (true, job.redact(&String::from_utf8_lossy(&logs))),
        Err(e) => (false, job.redact(&e.logs)),
    };
    let mut files = BTreeMap::from([
        (
            "status.json".into(),
            serde_json::to_vec(&serde_json::json!({"success":success})).unwrap(),
        ),
        ("build.log".into(), logs.into_bytes()),
    ]);
    if success {
        if std::fs::metadata(&output)
            .map_err(|_| webdock_build::Error {
                code: "artifact_missing",
                logs: String::new(),
            })?
            .len()
            > MAX_OUTPUT
        {
            return Err(webdock_build::Error {
                code: "artifact_limit",
                logs: String::new(),
            });
        }
        let bytes = std::fs::read(output).unwrap();
        webdock_build::artifact::validate_oci(&bytes)?;
        if let Some(config) = frontend {
            files.insert(
                "artifact/frontend.tar".into(),
                config.pack(&rootfs, MAX_OUTPUT.saturating_sub(bytes.len() as u64))?,
            );
        }
        files.insert("artifact/image.tar".into(), bytes);
    }
    let result = Archive::from_files(files)?.encode()?;
    if result.len() as u64 > MAX_OUTPUT {
        return Err(webdock_build::Error {
            code: "artifact_limit",
            logs: String::new(),
        });
    }
    Ok(result)
}
fn main() {
    // Refuse host execution even if mistakenly installed on the control-plane node.
    if !Path::new("/sys/class/virtio-ports").is_dir() || !Path::new("/dev/vdb").exists() {
        eprintln!("Disposable build VM required.");
        std::process::exit(1);
    }
    let result = build().unwrap_or_else(|error| {
        Archive::from_files(BTreeMap::from([
            ("status.json".into(), b"{\"success\":false}".to_vec()),
            (
                "build.log".into(),
                format!("Build input or artifact validation failed: {}", error.code).into_bytes(),
            ),
        ]))
        .unwrap()
        .encode()
        .unwrap()
    });
    // Discover the named host-created virtio port; never accept a source-selected path.
    for entry in std::fs::read_dir("/sys/class/virtio-ports")
        .unwrap()
        .flatten()
    {
        if std::fs::read_to_string(entry.path().join("name"))
            .is_ok_and(|v| v.trim() == "webdock.result")
        {
            let device = Path::new("/dev").join(entry.file_name());
            std::fs::OpenOptions::new()
                .write(true)
                .open(device)
                .unwrap()
                .write_all(&result)
                .unwrap();
        }
    }
    let _ = Command::new("/sbin/poweroff").arg("-f").status();
}
