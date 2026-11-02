use std::{
    collections::BTreeMap,
    io::{Read, Write},
};
use webdock_build::artifact::{
    Archive, MAX_OUTPUT, MAX_RETAINED_IMAGE, validate_retained_image_size,
};

#[derive(Default)]
struct ByteCount(u64);
impl Write for ByteCount {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn envelope_size(size: u64) -> u64 {
    let mut builder = tar::Builder::new(ByteCount::default());
    let mut header = tar::Header::new_ustar();
    header.set_path("image.tar").unwrap();
    header.set_size(size);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append(&header, std::io::repeat(0).take(size))
        .unwrap();
    builder.into_inner().unwrap().0
}
#[test]
fn actual_ustar_padding_reserves_header_and_two_end_blocks_at_maximum() {
    for size in [0, 1, 511, 512, 513, 1024] {
        let archive = Archive::from_files(BTreeMap::from([("image.tar".into(), vec![1; size])]))
            .unwrap()
            .encode()
            .unwrap();
        assert_eq!(archive.len() as u64, envelope_size(size as u64));
    }
    assert_eq!(envelope_size(MAX_RETAINED_IMAGE), MAX_OUTPUT);
    assert_eq!(envelope_size(MAX_RETAINED_IMAGE + 1), MAX_OUTPUT + 512);
}
#[cfg(feature = "kubernetes-job")]
#[test]
fn oversized_sparse_runner_output_reports_actionable_artifact_limit_before_reading() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("image.tar");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_RETAINED_IMAGE + 1).unwrap();
    drop(file);
    assert_eq!(
        webdock_build::container_runner::read_input_file(&path, MAX_RETAINED_IMAGE)
            .unwrap_err()
            .code,
        "artifact_limit"
    );
}

#[test]
fn trusted_retention_rejects_the_first_excess_byte_with_no_envelope_allocation() {
    assert_eq!(MAX_RETAINED_IMAGE, MAX_OUTPUT - 1536);
    assert!(validate_retained_image_size(MAX_RETAINED_IMAGE).is_ok());
    for size in [MAX_RETAINED_IMAGE + 1, MAX_OUTPUT, u64::MAX] {
        assert_eq!(
            validate_retained_image_size(size).unwrap_err().code,
            "artifact_limit"
        );
    }
}
