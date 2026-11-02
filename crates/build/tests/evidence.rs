use ed25519_dalek::SigningKey;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, os::unix::fs::PermissionsExt};
use webdock_build::{
    artifact::{Archive, ArtifactStore, MAX_OUTPUT},
    evidence::{BuildEvidence, MachineIdentity, assemble, validate},
};
fn legacy() -> Vec<u8> {
    let config = b"{}".to_vec();
    let hash = hex::encode(Sha256::digest(&config));
    let manifest=serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","size":2,"digest":format!("sha256:{hash}")},"layers":[]})).unwrap();
    let digest = hex::encode(Sha256::digest(&manifest));
    let index=serde_json::to_vec(&json!({"schemaVersion":2,"manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","size":manifest.len(),"digest":format!("sha256:{digest}")}]})).unwrap();
    let image = Archive::from_files(BTreeMap::from([
        (
            "oci-layout".into(),
            br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec(),
        ),
        ("index.json".into(), index),
        (format!("blobs/sha256/{hash}"), config),
        (format!("blobs/sha256/{digest}"), manifest),
    ]))
    .unwrap()
    .encode()
    .unwrap();
    Archive::from_files(BTreeMap::from([("image.tar".into(), image)]))
        .unwrap()
        .encode()
        .unwrap()
}
fn input() -> BuildEvidence {
    BuildEvidence {
        customer_id: "100".into(),
        project_id: "300".into(),
        build_id: "200".into(),
        generation: 1,
        source_commit: "a".repeat(40),
        recipe: "dockerfile".into(),
        root_directory: ".".into(),
        machine: MachineIdentity {
            image_sha256: "b".repeat(64),
            kernel_sha256: None,
            initrd_sha256: None,
        },
    }
}
fn source() -> Vec<u8> {
    Archive::from_files(BTreeMap::from([
        ("Dockerfile".into(), b"FROM scratch\n".to_vec()),
        ("web/index.html".into(), b"hello".to_vec()),
    ]))
    .unwrap()
    .encode()
    .unwrap()
}
#[test]
fn retained_bundle_binds_exact_source_scope_inventory_and_expected_signer() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let raw = legacy();
    let source = source();
    let bytes = assemble(&raw, &source, &input(), Some(&key)).unwrap();
    let archive = Archive::read(&bytes, MAX_OUTPUT).unwrap();
    assert_eq!(archive.files()["source.tar"], source);
    assert_eq!(
        archive.files()["image.tar"],
        Archive::read(&raw, MAX_OUTPUT).unwrap().files()["image.tar"]
    );
    let evidence = validate(&archive, "100", "200", 1, Some(&key.verifying_key()))
        .unwrap()
        .unwrap();
    assert_eq!(evidence.build.project_id, "300");
    assert!(validate(&archive, "101", "200", 1, None).is_err());
    assert!(validate(&archive, "100", "201", 1, None).is_err());
    assert!(validate(&archive, "100", "200", 2, None).is_err());
    assert!(
        validate(
            &archive,
            "100",
            "200",
            1,
            Some(&SigningKey::from_bytes(&[8; 32]).verifying_key())
        )
        .is_err()
    );
    let inventory: serde_json::Value =
        serde_json::from_slice(&archive.files()["source.spdx.json"]).unwrap();
    assert_eq!(inventory["files"].as_array().unwrap().len(), 2);
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/spdx-2.3.1.schema.json")).unwrap();
    jsonschema::validator_for(&schema)
        .unwrap()
        .validate(&inventory)
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = ArtifactStore::open(dir.path().into(), MAX_OUTPUT).unwrap();
    let receipt = store.put("100", "200", 1, &bytes).unwrap();
    assert_eq!(receipt.format_version().unwrap(), 1);
    assert!(receipt.storage_key().ends_with(".e1.tar"));
    assert_eq!(store.read(&receipt).unwrap(), bytes);
}
#[test]
fn rejects_tampered_companions_unknown_files_and_guest_supplied_evidence() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let bytes = assemble(&legacy(), &source(), &input(), Some(&key)).unwrap();
    let original = Archive::read(&bytes, MAX_OUTPUT).unwrap().into_files();
    for name in [
        "source.tar",
        "image.tar",
        "provenance.json",
        "source.spdx.json",
        "signature.json",
    ] {
        let mut files = original.clone();
        files.get_mut(name).unwrap()[0] ^= 1;
        let archive = Archive::from_files(files).unwrap();
        assert!(validate(&archive, "100", "200", 1, None).is_err(), "{name}");
    }
    let mut files = original.clone();
    files.insert("unexpected".into(), vec![1]);
    assert!(validate(&Archive::from_files(files).unwrap(), "100", "200", 1, None).is_err());
    assert!(assemble(&bytes, &source(), &input(), None).is_err());
    let mut bad = input();
    bad.machine.initrd_sha256 = Some("a".repeat(64));
    assert!(assemble(&legacy(), &source(), &bad, None).is_err());
}
#[test]
fn legacy_is_readable_but_never_satisfies_a_required_signature() {
    let archive = Archive::read(&legacy(), MAX_OUTPUT).unwrap();
    assert!(validate(&archive, "100", "200", 1, None).unwrap().is_none());
    assert!(
        validate(
            &archive,
            "100",
            "200",
            1,
            Some(&SigningKey::from_bytes(&[8; 32]).verifying_key())
        )
        .is_err()
    );
    let bytes = assemble(&legacy(), &source(), &input(), None).unwrap();
    let archive = Archive::read(&bytes, MAX_OUTPUT).unwrap();
    assert!(validate(&archive, "100", "200", 1, None).unwrap().is_some());
    assert!(
        validate(
            &archive,
            "100",
            "200",
            1,
            Some(&SigningKey::from_bytes(&[8; 32]).verifying_key())
        )
        .is_err()
    );
}

#[test]
fn well_formed_signature_and_provenance_tampering_reaches_crypto_verification() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let original = Archive::read(
        &assemble(&legacy(), &source(), &input(), Some(&key)).unwrap(),
        MAX_OUTPUT,
    )
    .unwrap()
    .into_files();
    let mut files = original.clone();
    let mut signature: serde_json::Value =
        serde_json::from_slice(&files["signature.json"]).unwrap();
    let value = signature["signature"].as_str().unwrap();
    signature["signature"] = json!(format!(
        "{}{}",
        if value.starts_with('0') { "1" } else { "0" },
        &value[1..]
    ));
    files.insert(
        "signature.json".into(),
        serde_json::to_vec(&signature).unwrap(),
    );
    assert_eq!(
        validate(&Archive::from_files(files).unwrap(), "100", "200", 1, None)
            .unwrap_err()
            .code,
        "artifact_signature_invalid"
    );
    let mut files = original;
    let mut provenance: serde_json::Value =
        serde_json::from_slice(&files["provenance.json"]).unwrap();
    provenance["build"]["projectID"] = json!("301");
    files.insert(
        "provenance.json".into(),
        serde_json::to_vec(&provenance).unwrap(),
    );
    assert_eq!(
        validate(&Archive::from_files(files).unwrap(), "100", "200", 1, None)
            .unwrap_err()
            .code,
        "artifact_signature_invalid"
    );
}

#[test]
fn escaped_source_filenames_cannot_construct_an_unreadable_inventory() {
    let component = "\u{1}".repeat(240);
    let mut files = BTreeMap::from([("Dockerfile".into(), b"FROM scratch\n".to_vec())]);
    for index in 0..22_000 {
        files.insert(format!("{component}/{component}/{index:05}"), Vec::new());
    }
    let archive = Archive::from_files(files).unwrap();
    let source = archive.encode().unwrap();
    Archive::read(&source, webdock_build::artifact::MAX_SOURCE).unwrap();
    let guest = archive
        .guest_request(b"{}".to_vec())
        .unwrap()
        .encode()
        .unwrap();
    Archive::read(&guest, webdock_build::artifact::MAX_SOURCE).unwrap();
    assert!(
        matches!(assemble(&legacy(),&source,&input(),None),Err(error) if error.code=="artifact_inventory_limit"),
        "Oversized inventory must be rejected before returning a bundle"
    );
}

#[test]
fn legacy_file_identity_remains_distinct_from_evidence_bundles() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = ArtifactStore::open(dir.path().into(), MAX_OUTPUT).unwrap();
    let bytes = legacy();
    let saved = store.put("100", "200", 1, &bytes).unwrap();
    assert_eq!(saved.format_version().unwrap(), 0);
    assert_eq!(
        saved.storage_key(),
        format!("100/200/1/{}.tar", hex::encode(Sha256::digest(&bytes)))
    );
    assert_eq!(store.read(&saved).unwrap(), bytes);
}

#[test]
fn load_rejects_legacy_and_evidence_bytes_relabelled_as_each_other() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = ArtifactStore::open(dir.path().into(), MAX_OUTPUT).unwrap();
    for (bytes, wrong_suffix) in [
        (legacy(), "e1.tar"),
        (
            assemble(&legacy(), &source(), &input(), None).unwrap(),
            "tar",
        ),
    ] {
        let saved = store.put("100", "200", 1, &bytes).unwrap();
        let wrong = format!(
            "100/200/1/{}.{}",
            hex::encode(Sha256::digest(&bytes)),
            wrong_suffix
        );
        std::fs::copy(
            dir.path().join(saved.storage_key()),
            dir.path().join(&wrong),
        )
        .unwrap();
        assert!(
            matches!(store.load(wrong, saved.digest().into(), saved.size(), saved.image_digest().into()), Err(e) if e.code == "artifact_format_mismatch")
        );
    }
}

fn frontend_inputs() -> (Vec<u8>, Vec<u8>, BuildEvidence, Vec<u8>) {
    let frontend = Archive::from_files(BTreeMap::from([(
        "index.html".into(),
        b"<p>retained frontend</p>".to_vec(),
    )]))
    .unwrap()
    .encode()
    .unwrap();
    let mut guest = Archive::read(&legacy(), MAX_OUTPUT).unwrap().into_files();
    guest.insert("frontend.tar".into(), frontend.clone());
    let mut source_files = Archive::read(&source(), MAX_OUTPUT).unwrap().into_files();
    source_files.insert(
        ".webdock/frontend.json".into(),
        br#"{"version":1,"directory":"app/dist"}"#.to_vec(),
    );
    let source = Archive::from_files(source_files).unwrap().encode().unwrap();
    let mut build = input();
    build.recipe = "dockerfile-frontend".into();
    (
        Archive::from_files(guest).unwrap().encode().unwrap(),
        source,
        build,
        frontend,
    )
}

#[test]
fn frontend_bundle_binds_exact_output_and_source_configuration_in_format_two() {
    let (guest, source, build, frontend) = frontend_inputs();
    let key = SigningKey::from_bytes(&[7; 32]);
    let bytes = assemble(&guest, &source, &build, Some(&key)).unwrap();
    let archive = Archive::read(&bytes, MAX_OUTPUT).unwrap();
    assert_eq!(archive.files()["frontend.tar"], frontend);
    let provenance = validate(&archive, "100", "200", 1, Some(&key.verifying_key()))
        .unwrap()
        .unwrap();
    assert_eq!(provenance.version, 2);
    {
        use ed25519_dalek::Signer;
        let mut wrong_domain = archive.files().clone();
        let mut message = b"webdock.build.evidence.v1\0".to_vec();
        message.extend(Sha256::digest(&wrong_domain["provenance.json"]));
        let mut signature: serde_json::Value =
            serde_json::from_slice(&wrong_domain["signature.json"]).unwrap();
        signature["signature"] = json!(hex::encode(key.sign(&message).to_bytes()));
        wrong_domain.insert(
            "signature.json".into(),
            serde_json::to_vec(&signature).unwrap(),
        );
        assert_eq!(
            validate(
                &Archive::from_files(wrong_domain).unwrap(),
                "100",
                "200",
                1,
                None
            )
            .unwrap_err()
            .code,
            "artifact_signature_invalid"
        );
    }
    let json: serde_json::Value =
        serde_json::from_slice(&archive.files()["provenance.json"]).unwrap();
    assert_eq!(json["frontend"]["directory"], "app/dist");
    assert_eq!(
        json["frontend"]["output"]["sha256"],
        hex::encode(Sha256::digest(&frontend))
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = ArtifactStore::open(dir.path().into(), MAX_OUTPUT).unwrap();
    let saved = store.put("100", "200", 1, &bytes).unwrap();
    assert_eq!(saved.format_version().unwrap(), 2);
    assert!(saved.storage_key().ends_with(".e2.tar"));
    let retained =
        webdock_build::frontend::retained(&store, &saved, &build, Some(&key.verifying_key()))
            .unwrap();
    assert_eq!(retained.files()["index.html"], b"<p>retained frontend</p>");
    let mut other = build.clone();
    other.project_id = "301".into();
    assert!(webdock_build::frontend::retained(&store, &saved, &other, None).is_err());
    other = build.clone();
    other.source_commit = "c".repeat(40);
    assert!(webdock_build::frontend::retained(&store, &saved, &other, None).is_err());
    assert_eq!(
        store
            .load(
                saved.storage_key().into(),
                saved.digest().into(),
                saved.size(),
                saved.image_digest().into()
            )
            .unwrap()
            .digest(),
        saved.digest()
    );
    for suffix in ["tar", "e1.tar", "e9.tar"] {
        let wrong = format!("100/200/1/{}.{suffix}", hex::encode(Sha256::digest(&bytes)));
        std::fs::copy(
            dir.path().join(saved.storage_key()),
            dir.path().join(&wrong),
        )
        .unwrap();
        assert!(
            store
                .load(
                    wrong,
                    saved.digest().into(),
                    saved.size(),
                    saved.image_digest().into()
                )
                .is_err()
        );
    }
    assert!(
        assemble(&guest, &source, &input(), None).is_err(),
        "plain dockerfile must reject frontend guest metadata"
    );
    assert!(
        assemble(&legacy(), &source, &build, None).is_err(),
        "frontend recipe requires actual files"
    );
    let mut files = archive.into_files();
    files.get_mut("frontend.tar").unwrap()[512] ^= 1;
    assert!(validate(&Archive::from_files(files).unwrap(), "100", "200", 1, None).is_err());
}

#[test]
fn frontend_evidence_cannot_relabel_configuration_recipe_or_version() {
    let (guest, source, build, _) = frontend_inputs();
    let original = Archive::read(
        &assemble(&guest, &source, &build, None).unwrap(),
        MAX_OUTPUT,
    )
    .unwrap()
    .into_files();
    for (pointer, value) in [
        ("/frontend/directory", json!("other")),
        ("/build/recipe", json!("dockerfile")),
        ("/version", json!(1)),
    ] {
        let mut files = original.clone();
        let mut provenance: serde_json::Value =
            serde_json::from_slice(&files["provenance.json"]).unwrap();
        *provenance.pointer_mut(pointer).unwrap() = value;
        files.insert(
            "provenance.json".into(),
            serde_json::to_vec(&provenance).unwrap(),
        );
        assert!(
            validate(&Archive::from_files(files).unwrap(), "100", "200", 1, None).is_err(),
            "{pointer}"
        );
    }
}
