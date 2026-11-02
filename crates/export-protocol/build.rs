fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto = "../../proto/webdock/export/v1/export.proto";
    println!("cargo:rerun-if-changed={proto}");
    let mut config = prost_build::Config::new();
    config.skip_debug(["webdock.export.v1.DatabaseMaterial"]);
    tonic_prost_build::configure().compile_with_config(config, &[proto], &["../../proto"])?;
    Ok(())
}
