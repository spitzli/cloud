fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto = "../../proto/webdock/agent/v1/agent.proto";
    println!("cargo:rerun-if-changed={proto}");
    let recovery = "../../proto/webdock/agent/v1/recovery.proto";
    println!("cargo:rerun-if-changed={recovery}");
    tonic_prost_build::configure().compile_protos(&[proto, recovery], &["../../proto"])?;
    Ok(())
}
