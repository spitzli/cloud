use webdock_plugin_protocol::Manifest;

#[test]
fn authoring_release_keeps_the_scoped_read_only_runtime_contract() {
    let root = Manifest::parse(include_bytes!("../manifest.json")).unwrap();
    let fixture = Manifest::parse(include_bytes!("manifest.json")).unwrap();
    webdock_lumaserv_compute::validate_manifest(&root).unwrap();
    assert_eq!(root.version(), env!("CARGO_PKG_VERSION"));
    for field in [
        "protocolVersion",
        "sdkVersion",
        "dataProtocolVersion",
        "capabilities",
        "httpBindings",
        "configuration",
        "services",
        "actions",
        "jobs",
        "offerings",
        "selfService",
        "areas",
        "api",
        "mcp",
    ] {
        assert_eq!(root.as_json()[field], fixture.as_json()[field], "{field}");
    }
}
