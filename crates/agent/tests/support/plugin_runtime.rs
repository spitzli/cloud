use webdock_agent_protocol::plugin::*;
pub fn claim() -> PluginRuntimeClaim {
    let material = PluginRuntimeMaterial {
        files: MATERIAL_FILES
            .iter()
            .map(|name| {
                (
                    (*name).into(),
                    if *name == "manifest.json" {
                        "{}".into()
                    } else {
                        "fixture".into()
                    },
                )
            })
            .collect(),
    };
    PluginRuntimeClaim {
        spec: PluginRuntimeSpec {
            version: 1,
            platform_id: "fixture".into(),
            plugin_id: "postgres".into(),
            runtime_generation: 1,
            configuration_revision: 1,
            cluster_id: "123".into(),
            manifest_sha256: sha256(b"{}"),
            material_sha256: material.fingerprint().unwrap(),
            image: format!("registry.example/postgres@sha256:{}", "a".repeat(64)),
            namespace: PluginRuntimeSpec::namespace_name("fixture", "postgres"),
            service: PluginRuntimeSpec::service_name().into(),
            runtime_host: "pg.plugins.example".into(),
            port: 3443,
            entry_point: "plugins".into(),
            cpu_millicores: 100,
            memory_bytes: 134217728,
            egress: vec![PluginRuntimeEgress {
                cidr: "192.0.2.10/32".into(),
                port: 443,
            }],
            host_aliases: vec![],
        },
        lease: PluginRuntimeLease {
            operation_id: "456".into(),
            generation: 1,
            token: "b".repeat(64),
            lease_until: webdock_supervisor::timestamp().unwrap() + 120,
            agent_generation: 1,
            action: PluginRuntimeAction::Apply,
        },
        material,
        checkpoint: None,
    }
}
