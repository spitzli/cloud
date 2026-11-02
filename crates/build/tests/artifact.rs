use std::collections::BTreeMap;
use webdock_build::artifact::{Archive, validate_oci};
fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = tar::Builder::new(Vec::new());
    for (path, bytes) in files {
        let mut h = tar::Header::new_ustar();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        out.append_data(&mut h, path, *bytes).unwrap();
    }
    out.into_inner().unwrap()
}
#[test]
fn validates_archive_before_any_extraction() {
    let good = archive(&[("source/Dockerfile", b"FROM scratch\n")]);
    let parsed = Archive::read(&good, 1024).unwrap();
    assert_eq!(parsed.files()["source/Dockerfile"], b"FROM scratch\n");
    assert!(Archive::read(&good, 4).is_err());
    assert!(Archive::read(&archive(&[("same", b"a"), ("same", b"b")]), 1024).is_err());
    let mut link = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_ustar();
    h.set_entry_type(tar::EntryType::Symlink);
    h.set_size(0);
    h.set_mode(0o777);
    h.set_link_name("/etc/passwd").unwrap();
    h.set_cksum();
    link.append_data(&mut h, "secret", &b""[..]).unwrap();
    assert!(Archive::read(&link.into_inner().unwrap(), 1024).is_err());
    let mut trailing = good;
    trailing.extend(b"hidden payload");
    assert!(Archive::read(&trailing, 1024).is_err());
    assert!(Archive::from_files(BTreeMap::from([("../escape".into(), vec![])])).is_err());
}
#[test]
fn validates_recursive_oci_digests_and_rejects_missing_blob() {
    use serde_json::json;
    use sha2::{Digest, Sha256};
    let config = b"{}";
    let hash = hex::encode(Sha256::digest(config));
    let manifest=serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","size":2,"digest":format!("sha256:{hash}")},"layers":[]})).unwrap();
    let digest = hex::encode(Sha256::digest(&manifest));
    let index=serde_json::to_vec(&json!({"schemaVersion":2,"manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","size":manifest.len(),"digest":format!("sha256:{digest}")}]})).unwrap();
    let bytes = archive(&[
        ("oci-layout", br#"{"imageLayoutVersion":"1.0.0"}"#),
        ("index.json", &index),
        (&format!("blobs/sha256/{digest}"), &manifest),
        (&format!("blobs/sha256/{hash}"), config),
    ]);
    assert_eq!(validate_oci(&bytes).unwrap(), format!("sha256:{digest}"));
    let mut files = Archive::read(&bytes, 4096).unwrap().into_files();
    files.remove(&format!("blobs/sha256/{hash}"));
    assert!(validate_oci(&Archive::from_files(files).unwrap().encode().unwrap()).is_err());
}
#[test]
fn executable_source_mode_survives_guest_pack_and_private_extraction() {
    use std::os::unix::fs::PermissionsExt;
    let mut tar = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_ustar();
    h.set_mode(0o4755);
    h.set_size(7);
    h.set_cksum();
    tar.append_data(&mut h, "script", &b"echo ok"[..]).unwrap();
    let source = Archive::read(&tar.into_inner().unwrap(), 100).unwrap();
    let request = source.guest_request(b"{}".to_vec()).unwrap();
    let parsed = Archive::read(&request.encode().unwrap(), 100).unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    parsed.extract_private(dir.path()).unwrap();
    assert_eq!(
        std::fs::metadata(dir.path().join("source/script"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o700
    );
}
#[test]
fn long_utf8_source_paths_roundtrip_without_gnu_extensions() {
    let path = format!("{}/{}", "é".repeat(75), "x".repeat(150));
    let files = BTreeMap::from([(path.clone(), b"contents".to_vec())]);
    let archive = Archive::from_files(files).unwrap().encode().unwrap();
    assert_eq!(
        Archive::read(&archive, 1024).unwrap().files()[&path],
        b"contents"
    );
}
