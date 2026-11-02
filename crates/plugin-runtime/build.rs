fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto = "../../proto/webdock/plugin/v1/plugin.proto";
    println!("cargo:rerun-if-changed={proto}");
    tonic_prost_build::configure().compile_protos(&[proto], &["../../proto"])?;
    Ok(())
}
