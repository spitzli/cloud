//! Apps on the cluster through `plugin.workloads`: one unit per app with a Deployment, a Service,
//! a Secret for its environment and a Traefik route. Core pins the namespace and owner labels;
//! the cluster's agent applies the objects and reports readiness, pod problems and recent logs.
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;
use webdock_plugin_protocol::PluginError;

pub use webdock_plugin_kit::workloads::{HostWorkloads, Workloads};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlacementConfig {
    /// Apps answer on `<name>-<id>.<domain>`.
    pub domain: String,
    /// How customers reach that host, with `{host}` in it, e.g. `http://{host}:55480`.
    pub public_url: String,
    /// Traefik entry points of the route, e.g. `["websecure"]`.
    pub entry_points: Vec<String>,
    /// cert-manager ClusterIssuer for each app's certificate; the route then serves HTTPS with it.
    #[serde(default)]
    pub issuer: Option<String>,
}
pub struct Spec<'a> {
    pub id: &'a Uuid,
    pub project: &'a str,
    pub name: &'a str,
    pub image: &'a str,
    pub port: u16,
    pub memory_mb: u32,
    pub env: &'a [(String, String)],
    pub running: bool,
    /// Changing it rolls the pods.
    pub restart: &'a str,
    /// The customer's verified domains the app also answers on.
    pub domains: &'a [String],
}
pub struct Observed {
    pub status: &'static str,
    pub error: Option<String>,
    pub logs: Vec<String>,
}
pub struct Placement {
    workloads: Box<dyn Workloads>,
    config: PlacementConfig,
}
pub fn unit(id: &Uuid) -> String {
    format!("app-{}", id.simple())
}
/// `<name>-<first 8 of the id>`: readable, unique, a DNS label.
pub fn host(config: &PlacementConfig, id: &Uuid, name: &str) -> String {
    let lowered: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let mut slug = lowered
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    slug.truncate(40);
    let slug = slug.trim_end_matches('-');
    let short = &id.simple().to_string()[..8];
    if slug.is_empty() {
        format!("app-{short}.{}", config.domain)
    } else {
        format!("{slug}-{short}.{}", config.domain)
    }
}
impl Placement {
    pub fn new(workloads: Box<dyn Workloads>, config: PlacementConfig) -> Self {
        Self { workloads, config }
    }
    pub fn domain(&self) -> &str {
        &self.config.domain
    }
    pub fn host(&self, id: &Uuid, name: &str) -> String {
        host(&self.config, id, name)
    }
    pub fn url(&self, id: &Uuid, name: &str) -> String {
        self.config
            .public_url
            .replace("{host}", &host(&self.config, id, name))
    }
    pub fn objects(&self, spec: &Spec<'_>) -> Value {
        let unit = unit(spec.id);
        let memory = format!("{}Mi", spec.memory_mb);
        let cpu = format!("{}m", (spec.memory_mb / 4).max(50));
        let env: serde_json::Map<String, Value> = spec
            .env
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        let selector = json!({"app": unit});
        let hosts: Vec<String> = std::iter::once(host(&self.config, spec.id, spec.name))
            .chain(spec.domains.iter().cloned())
            .collect();
        let rule = hosts
            .iter()
            .map(|h| format!("Host(`{h}`)"))
            .collect::<Vec<_>>()
            .join(" || ");
        let mut objects = json!([
            {"apiVersion": "v1", "kind": "Secret", "metadata": {"name": format!("{unit}-env")}, "type": "Opaque", "stringData": env},
            {"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": unit, "labels": selector},
             "spec": {"replicas": if spec.running { 1 } else { 0 }, "selector": {"matchLabels": selector},
              "template": {"metadata": {"labels": selector, "annotations": {"restarted": spec.restart}},
               "spec": {"containers": [{"name": "app", "image": spec.image,
                 "ports": [{"containerPort": spec.port}],
                 "envFrom": [{"secretRef": {"name": format!("{unit}-env")}}],
                 // CPU is reserved in step with memory (a quarter millicore per MiB) and capped at one core.
                 "resources": {"limits": {"memory": memory, "cpu": "1"}, "requests": {"memory": memory, "cpu": cpu}},
                 // Ready once the port accepts connections, so a new image takes over only when it answers.
                 "readinessProbe": {"tcpSocket": {"port": spec.port}, "periodSeconds": 5, "failureThreshold": 3},
                 "securityContext": {"allowPrivilegeEscalation": false}}]}}}},
            {"apiVersion": "v1", "kind": "Service", "metadata": {"name": unit},
             "spec": {"selector": selector, "ports": [{"port": 80, "targetPort": spec.port}]}},
            {"apiVersion": "traefik.io/v1alpha1", "kind": "IngressRoute", "metadata": {"name": unit},
             "spec": {"entryPoints": self.config.entry_points, "routes": [{"kind": "Rule",
               "match": rule,
               "services": [{"name": unit, "port": 80}]}]}},
        ]);
        if let Some(issuer) = &self.config.issuer {
            let tls = format!("{unit}-tls");
            objects[3]["spec"]["tls"] = json!({"secretName": tls});
            if let Some(list) = objects.as_array_mut() {
                list.push(json!({"apiVersion": "cert-manager.io/v1", "kind": "Certificate", "metadata": {"name": tls},
                    "spec": {"secretName": tls, "dnsNames": hosts, "issuerRef": {"name": issuer, "kind": "ClusterIssuer"}}}));
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
    /// The app's state in plain words, from what the cluster's agent last saw.
    pub async fn observe(&self, id: &Uuid, running: bool) -> Result<Observed, PluginError> {
        let seen = self
            .workloads
            .call("observe", json!({"unit": unit(id)}))
            .await?;
        Ok(status(&seen, running))
    }
}
pub fn status(seen: &Value, running: bool) -> Observed {
    let logs = seen["observed"]["logs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let (status, error) = if seen["removed"] == true || seen["removing"] == true {
        ("missing", None)
    } else if let Some(error) = seen["error"].as_str() {
        (
            "failed",
            Some(format!("The cluster refused the app: {error}")),
        )
    } else if seen["appliedRevision"].as_i64() < seen["revision"].as_i64() {
        ("deploying", None)
    } else if !running {
        ("stopped", None)
    } else if let Some(reason) = seen["observed"]["problems"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|p| p["reason"].as_str())
    {
        let message = match reason {
            "ErrImagePull" | "ImagePullBackOff" | "InvalidImageName" => {
                "The image could not be pulled. Check the image name.".to_owned()
            }
            "CrashLoopBackOff" => {
                "The app keeps stopping right after it starts. See the logs.".to_owned()
            }
            other => format!("The app cannot start ({other})."),
        };
        ("failed", Some(message))
    } else if seen["observed"]["ready"] == true {
        ("running", None)
    } else {
        ("deploying", None)
    };
    Observed {
        status,
        error,
        logs,
    }
}
