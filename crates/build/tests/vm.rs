//! Explicit disposable local KVM acceptance. Never runs customer code on the host.
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf};
use webdock_build::{
    artifact::Archive,
    execution::{GuestRequest, Limits, VmImage},
};
fn digest(path: &std::path::Path) -> String {
    hex::encode(Sha256::digest(std::fs::read(path).unwrap()))
}
#[test]
#[ignore = "requires explicitly prepared disposable local VM"]
fn real_isolated_oci_build_and_failure() {
    assert_eq!(std::env::var("WEBDOCK_BUILD_VM_TEST").as_deref(), Ok("1"));
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.runtime-test/build-vm")
        .canonicalize()
        .unwrap();
    // The expected hashes come from the separately reviewed preparation record, not the files being validated.
    let hashes = std::fs::read_to_string(root.join("SHA256SUMS")).unwrap();
    let expected = |name: &str| {
        hashes
            .lines()
            .find(|l| l.ends_with(&format!("/{name}")))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned()
    };
    let image = VmImage::new(root.join("base.raw"), expected("base.raw"))
        .unwrap()
        .with_boot(
            root.join("kernel"),
            expected("kernel"),
            root.join("initrd"),
            expected("initrd"),
        )
        .unwrap();
    let scratch = tempfile::tempdir_in(&root).unwrap();
    let mut job = GuestRequest {
        recipe: "dockerfile".into(),
        root_directory: ".".into(),
        build_environment: BTreeMap::new(),
        limits: Limits {
            cpu: 2,
            memory_mi_b: 2048,
            disk_bytes: 24 * 1024 * 1024 * 1024,
            duration_seconds: 180,
        },
    };
    let common = "FROM scratch\nCOPY --chmod=755 busybox /bin/busybox\nCOPY --chmod=755 musl /lib/ld-musl-x86_64.so.1\nSHELL [\"/bin/busybox\",\"sh\",\"-c\"]\n";
    let run = |job: &GuestRequest, script: &str| {
        let files = BTreeMap::from([
            (
                "Dockerfile".into(),
                format!("{common}{script}").into_bytes(),
            ),
            (
                "busybox".into(),
                std::fs::read(root.join("rootfs/bin/busybox")).unwrap(),
            ),
            (
                "musl".into(),
                std::fs::read(root.join("rootfs/lib/ld-musl-x86_64.so.1")).unwrap(),
            ),
        ]);
        let archive = Archive::from_files(files).unwrap().encode().unwrap();
        image.execute(job, &archive, scratch.path(), || Ok(()))
    };
    let good=run(&job,"RUN --mount=type=cache,target=/cache test ! -e /cache/previous && /bin/busybox touch /cache/previous && test ! -e /etc/webdock-build-worker/credential && ! /bin/busybox wget -T 1 -q -O - http://169.254.169.254/\nENTRYPOINT [\"/bin/busybox\",\"true\"]\n").unwrap_or_else(|e|panic!("{}: {}",e.code,e.logs));
    std::fs::write(root.join("verified-artifact.tar"), &good.artifact).unwrap();
    assert!(good.image_digest.starts_with("sha256:"));
    assert!(!good.artifact.is_empty());
    run(
        &job,
        "RUN --mount=type=cache,target=/cache test ! -e /cache/previous\n",
    )
    .unwrap_or_else(|e| panic!("{}: {}", e.code, e.logs));
    let failed = run(&job, "RUN echo authored-failure-visible; exit 17\n")
        .err()
        .expect("build failure");
    assert_eq!(failed.code, "build_failed");
    assert!(failed.logs.contains("authored-failure-visible"));
    let logs = run(&job, "RUN /bin/busybox yes bounded-log-flood\n")
        .err()
        .expect("log limit");
    assert_eq!(logs.code, "build_failed");
    assert!(logs.logs.len() <= 24_000);
    job.limits.duration_seconds = 30;
    let started = std::time::Instant::now();
    assert!(run(&job, "RUN /bin/busybox sleep 120\n").is_err());
    assert!(started.elapsed().as_secs() < 40);
    assert_eq!(digest(&root.join("base.raw")), expected("base.raw"));
    assert_eq!(scratch.path().read_dir().unwrap().count(), 0);
}
