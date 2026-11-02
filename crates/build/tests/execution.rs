use std::{
    process::Command,
    time::{Duration, Instant},
};
use webdock_build::execution::{MAX_LOG, run_bounded};

#[test]
fn frontend_requires_explicit_pinned_vm_capability_and_guest_output_matches_recipe() {
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use webdock_build::{
        artifact::Archive,
        execution::{GuestRequest, Limits, VmImage, decode_guest_output},
    };
    let request = GuestRequest {
        recipe: "dockerfile-frontend".into(),
        root_directory: ".".into(),
        build_environment: BTreeMap::new(),
        limits: Limits {
            cpu: 2,
            memory_mi_b: 2048,
            disk_bytes: 24 * 1024 * 1024 * 1024,
            duration_seconds: 180,
        },
    };
    request.validate().unwrap();
    let image_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(image_file.path(), b"not executed").unwrap();
    let image = VmImage::new(
        image_file.path().into(),
        hex::encode(Sha256::digest(b"not executed")),
    )
    .unwrap();
    assert_eq!(image.supported_recipes(), &["dockerfile"]);
    let scratch = tempfile::tempdir().unwrap();
    assert_eq!(
        image
            .execute(&request, b"", scratch.path(), || panic!(
                "must reject before lease or QEMU"
            ))
            .err()
            .unwrap()
            .code,
        "recipe_unavailable"
    );
    let image = image.with_frontend_output();
    assert_eq!(
        image.supported_recipes(),
        &["dockerfile", "dockerfile-frontend"]
    );
    let output = Archive::from_files(BTreeMap::from([
        ("status.json".into(), b"{\"success\":true}".to_vec()),
        (
            "artifact/image.tar".into(),
            b"unimportant: reject unexpected companion first".to_vec(),
        ),
        ("artifact/frontend.tar".into(), b"unexpected".to_vec()),
    ]))
    .unwrap()
    .encode()
    .unwrap();
    let mut plain = request.clone();
    plain.recipe = "dockerfile".into();
    assert_eq!(
        decode_guest_output(&plain, &output).err().unwrap().code,
        "invalid_result"
    );
}
#[test]
fn host_watchdog_kills_process_and_bounds_output() {
    let mut sleeper = Command::new("/bin/sleep");
    sleeper.arg("30");
    let started = Instant::now();
    let failed = run_bounded(&mut sleeper, Duration::from_millis(100), 1024, &mut || {
        Ok(())
    })
    .unwrap_err();
    assert_eq!(failed.code, "execution_timeout");
    assert!(started.elapsed() < Duration::from_secs(2));
    let mut noisy = Command::new("/usr/bin/yes");
    noisy.arg("authored-host-watchdog-fixture");
    let failed = run_bounded(&mut noisy, Duration::from_secs(2), 1024, &mut || Ok(())).unwrap_err();
    assert_eq!(failed.code, "log_limit");
    assert!(failed.logs.len() <= MAX_LOG);
    let mut revoked = Command::new("/bin/sleep");
    revoked.arg("30");
    let failed = run_bounded(&mut revoked, Duration::from_secs(2), 1024, &mut || {
        Err(webdock_build::Error {
            code: "lease_revoked",
            logs: String::new(),
        })
    })
    .unwrap_err();
    assert_eq!(failed.code, "lease_revoked");
}
#[test]
fn host_watchdog_limits_writable_file_growth() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("limited-output");
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            "exec /bin/dd if=/dev/zero of=\"$1\" bs=4096 count=1000",
            "fixture",
        ])
        .arg(&target);
    assert!(run_bounded(&mut command, Duration::from_secs(2), 1024, &mut || Ok(())).is_err());
    assert!(std::fs::metadata(target).unwrap().len() <= 1024);
}
