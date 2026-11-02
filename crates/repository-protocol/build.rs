fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/repository.proto");
    tonic_prost_build::configure().compile_protos(&["proto/repository.proto"], &["proto"])?;
    Ok(())
}
