//! Dedicated instances on the cluster through `plugin.workloads`: one CloudNativePG cluster per
//! resource. The customer role's password lives in a Secret the operator reconciles, so a rotation
//! is just a new apply. Customers connect through Traefik, routed by TLS server name.
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;
pub use webdock_plugin_kit::workloads::{HostWorkloads, Workloads};
use webdock_plugin_protocol::PluginError;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DedicatedConfig {
    /// Instances answer on `pg-<id>.<domain>`.
    pub domain: String,
    /// Port of Traefik's PostgreSQL entry point as customers reach it.
    pub port: u16,
    pub entry_points: Vec<String>,
    /// cert-manager ClusterIssuer for the instances' server certificates, so clients can verify
    /// them (`sslmode=verify-full`). Without one, instances keep the operator's own certificates.
    #[serde(default)]
    pub issuer: Option<String>,
}
pub fn image(version: &str) -> Option<&'static str> {
    // ponytail: major tags, not digests; pin digests for production installs.
    match version {
        "18" => Some("ghcr.io/cloudnative-pg/postgresql:18"),
        "17" => Some("ghcr.io/cloudnative-pg/postgresql:17"),
        _ => None,
    }
}
pub fn unit(id: &Uuid) -> String {
    format!("pg-{}", id.simple())
}
pub struct Spec<'a> {
    pub id: &'a Uuid,
    pub project: &'a str,
    pub version: &'a str,
    pub database: &'a str,
    pub role: &'a str,
    pub password: &'a str,
}
pub struct Placement {
    workloads: Box<dyn Workloads>,
    config: DedicatedConfig,
}
impl Placement {
    pub fn new(workloads: Box<dyn Workloads>, config: DedicatedConfig) -> Self {
        Self { workloads, config }
    }
    pub fn host(&self, id: &Uuid) -> String {
        format!("{}.{}", unit(id), self.config.domain)
    }
    pub fn port(&self) -> u16 {
        self.config.port
    }
    /// What customers put in `sslmode`.
    pub fn ssl_mode(&self) -> &'static str {
        if self.config.issuer.is_some() {
            "verify-full"
        } else {
            "require"
        }
    }
    pub fn objects(&self, spec: &Spec<'_>) -> Value {
        let unit = unit(spec.id);
        let secret = format!("{unit}-role");
        let mut objects = json!([
            // The operator watches labelled Secrets and applies a changed password to the role.
            {"apiVersion": "v1", "kind": "Secret", "metadata": {"name": secret, "labels": {"cnpg.io/reload": "true"}},
             "type": "kubernetes.io/basic-auth", "stringData": {"username": spec.role, "password": spec.password}},
            {"apiVersion": "postgresql.cnpg.io/v1", "kind": "Cluster", "metadata": {"name": unit},
             "spec": {"instances": 1, "imageName": image(spec.version), "storage": {"size": "1Gi"},
              "resources": {"limits": {"memory": "512Mi"}, "requests": {"memory": "512Mi", "cpu": "100m"}},
              "bootstrap": {"initdb": {"database": spec.database, "owner": spec.role, "secret": {"name": secret}}},
              "managed": {"roles": [{"name": spec.role, "login": true, "ensure": "present", "passwordSecret": {"name": secret}}]}}},
            {"apiVersion": "traefik.io/v1alpha1", "kind": "IngressRouteTCP", "metadata": {"name": unit},
             "spec": {"entryPoints": self.config.entry_points,
              "routes": [{"match": format!("HostSNI(`{}`)", self.host(spec.id)), "services": [{"name": format!("{unit}-rw"), "port": 5432}]}],
              "tls": {"passthrough": true}}},
        ]);
        if let Some(issuer) = &self.config.issuer {
            let tls = format!("{unit}-tls");
            objects[1]["spec"]["certificates"] =
                json!({"serverTLSSecret": tls, "serverCASecret": tls});
            if let Some(list) = objects.as_array_mut() {
                list.push(json!({"apiVersion": "cert-manager.io/v1", "kind": "Certificate", "metadata": {"name": tls},
                    "spec": {"secretName": tls, "dnsNames": [self.host(spec.id)], "issuerRef": {"name": issuer, "kind": "ClusterIssuer"}}}));
            }
        }
        objects
    }
    pub async fn deploy(&self, spec: &Spec<'_>) -> Result<(), PluginError> {
        self.workloads
            .call("apply", json!({"project": spec.project, "unit": unit(spec.id), "resource": spec.id.to_string(), "objects": self.objects(spec)}))
            .await?;
        Ok(())
    }
    pub async fn remove(&self, id: &Uuid) -> Result<(), PluginError> {
        self.workloads
            .call("remove", json!({"unit": unit(id)}))
            .await?;
        Ok(())
    }
    /// `creating`, `ready` or `failed`, from what the cluster's agent last saw.
    pub async fn observe(&self, id: &Uuid) -> Result<(&'static str, Option<String>), PluginError> {
        Ok(state(
            &self
                .workloads
                .call("observe", json!({"unit": unit(id)}))
                .await?,
        ))
    }
}
pub fn state(seen: &Value) -> (&'static str, Option<String>) {
    if seen["removed"] == true || seen["removing"] == true {
        ("failed", Some("The instance is gone".into()))
    } else if let Some(error) = seen["error"].as_str() {
        (
            "failed",
            Some(format!("The cluster refused the instance: {error}")),
        )
    } else if seen["observed"]["ready"] == true {
        // A rotation in flight keeps a running instance ready.
        ("ready", None)
    } else {
        ("creating", None)
    }
}
