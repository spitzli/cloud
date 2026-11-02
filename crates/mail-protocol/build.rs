fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/mail.proto");
    tonic_prost_build::configure().compile_protos(&["proto/mail.proto"], &["proto"])?;
    Ok(())
}
