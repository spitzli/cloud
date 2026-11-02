//! Explicit owned guest qualification, independent of Core/schema bootstrap.
use std::{collections::BTreeMap, path::PathBuf};
use webdock_build::{
    artifact::{Archive, MAX_OUTPUT},
    evidence::{BuildEvidence, assemble},
    execution::{GuestRequest, Limits, VmImage},
    frontend::{FrontendConfig, retained},
};

#[test]
#[ignore = "requires explicitly prepared, uniquely owned frontend VM"]
fn real_frontend_guest_retains_files_and_fails_closed_on_unsafe_or_oversized_outputs() {
    assert_eq!(
        std::env::var("WEBDOCK_FRONTEND_VM_TEST").as_deref(),
        Ok("1")
    );
    let root = PathBuf::from(std::env::var_os("WEBDOCK_FRONTEND_VM_ROOT").expect("owned VM path"));
    assert!(root.is_absolute());
    assert!(
        root.canonicalize()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("build-vm-frontend-")
    );
    let hashes = std::fs::read_to_string(root.join("SHA256SUMS")).unwrap();
    let expected = |name: &str| {
        hashes
            .lines()
            .find(|line| line.ends_with(&format!("/{name}")))
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
        .unwrap()
        .with_frontend_output();
    let scratch = tempfile::tempdir_in(&root).unwrap();
    let job = GuestRequest {
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
    let run = |script: &str, directory: &str| {
        let dockerfile = format!(
            "FROM scratch\nCOPY --chmod=755 busybox /bin/busybox\nCOPY --chmod=755 musl /lib/ld-musl-x86_64.so.1\nSHELL [\"/bin/busybox\",\"sh\",\"-c\"]\nRUN {script}\nENTRYPOINT [\"/bin/busybox\",\"true\"]\n"
        );
        let source = Archive::from_files(BTreeMap::from([
            ("Dockerfile".into(), dockerfile.into_bytes()),
            (
                "busybox".into(),
                std::fs::read(root.join("rootfs/bin/busybox")).unwrap(),
            ),
            (
                "musl".into(),
                std::fs::read(root.join("rootfs/lib/ld-musl-x86_64.so.1")).unwrap(),
            ),
            (
                ".webdock/frontend.json".into(),
                serde_json::to_vec(&FrontendConfig {
                    version: 1,
                    directory: directory.into(),
                })
                .unwrap(),
            ),
        ]))
        .unwrap()
        .encode()
        .unwrap();
        (
            image.execute(&job, &source, scratch.path(), || Ok(())),
            source,
        )
    };
    let started = std::time::Instant::now();
    let (good, source) = run(
        "/bin/busybox mkdir -p /app/dist/assets; printf '<p>owned frontend</p>' > /app/dist/index.html; printf 'console.log(42)' > /app/dist/assets/main.js",
        "app/dist",
    );
    let good = good.unwrap_or_else(|e| panic!("{}: {}", e.code, e.logs));
    let build = BuildEvidence {
        customer_id: "100".into(),
        project_id: "300".into(),
        build_id: "200".into(),
        generation: 1,
        source_commit: "a".repeat(40),
        recipe: job.recipe.clone(),
        root_directory: ".".into(),
        machine: image.identity(),
    };
    let bytes = assemble(&good.artifact, &source, &build, None).unwrap();
    std::fs::write(root.join("verified-frontend-e2.tar"), &bytes).unwrap();
    std::fs::write(root.join("verified-source.tar"), &source).unwrap();
    let local = tempfile::tempdir_in(&root).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(local.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store =
        webdock_build::artifact::ArtifactStore::open(local.path().into(), MAX_OUTPUT).unwrap();
    let artifact = store.put("100", "200", 1, &bytes).unwrap();
    let files = retained(&store, &artifact, &build, None)
        .unwrap()
        .into_files();
    assert_eq!(
        files,
        BTreeMap::from([
            ("index.html".into(), b"<p>owned frontend</p>".to_vec()),
            ("assets/main.js".into(), b"console.log(42)".to_vec()),
        ])
    );
    for (script, directory) in [
        (
            "/bin/busybox mkdir -p /app/dist; /bin/busybox ln -s /etc/passwd /app/dist/link",
            "app/dist",
        ),
        (
            "/bin/busybox mkdir -p /app/dist; /bin/busybox mkfifo /app/dist/fifo",
            "app/dist",
        ),
        (
            "/bin/busybox mkdir -p /app/dist; /bin/busybox truncate -s 536870912 /app/dist/oversized",
            "app/dist",
        ),
        ("/bin/busybox mkdir -p /app/dist", "missing"),
        ("/bin/busybox mkdir -p /app/dist", "../app/dist"),
    ] {
        let error = run(script, directory)
            .0
            .err()
            .expect("unsafe/oversized output must fail");
        assert_eq!(error.code, "build_failed", "{}: {}", error.code, error.logs);
        assert!(scratch.path().read_dir().unwrap().next().is_none());
    }
    assert!(scratch.path().read_dir().unwrap().next().is_none());
    println!(
        "Owned frontend guest qualification: {:.2}s; exact HTML/JS, e2 evidence, links/FIFO/output overflow/traversal/missing output rejected",
        started.elapsed().as_secs_f64()
    );
}
