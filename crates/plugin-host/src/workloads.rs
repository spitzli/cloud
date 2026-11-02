//! `plugin.workloads`: a plugin runtime states the Kubernetes objects one unit (usually one
//! resource) needs. Core checks them against the manifest and a fixed safety policy, pins them to
//! the installation's namespace in the project or customer and stamps owner labels; the cluster's agent
//! applies them and reports back. Calls authenticate the plugin runtime itself, not an invocation,
//! because provisioning outlasts one.
use super::*;
use webdock_plugin_protocol::WORKLOAD_KINDS;
use webdock_plugin_runtime::PeerIdentity;

const MAX_OBJECTS: usize = 32;
const MAX_BYTES: usize = 256 * 1024;
const OWNER: &str = "webdock.dev/";

fn dns_label(value: &str) -> bool {
    (1..=63).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}
/// `wd-<plugin>-<project>`; a plugin id too long for that is shortened to a hash. Plugins never
/// choose a namespace.
pub fn namespace(plugin: &str, project: &str) -> Result<String> {
    if project.is_empty() || project.len() > 20 || !project.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid("Invalid project"));
    }
    let value = format!("wd-{plugin}-{project}");
    let value = if dns_label(&value) {
        value
    } else {
        format!("wd-{}-{project}", &digest(plugin.as_bytes())[..12])
    };
    if !dns_label(&value) {
        return Err(invalid("Invalid workload namespace"));
    }
    Ok(value)
}
fn customer_namespace(plugin: &str, customer: &str) -> Result<String> {
    if !numeric_owner(customer) {
        return Err(invalid("Invalid customer"));
    }
    let value = format!("wd-{plugin}-c{customer}");
    Ok(if dns_label(&value) {
        value
    } else {
        format!("wd-{}-c{customer}", &digest(plugin.as_bytes())[..12])
    })
}
fn numeric_owner(value: &str) -> bool {
    !value.is_empty() && value.len() <= 20 && value.bytes().all(|b| b.is_ascii_digit())
}
fn object_map<'a>(
    value: &'a Value,
    what: &'static str,
) -> Result<&'a serde_json::Map<String, Value>> {
    value.as_object().ok_or_else(|| invalid(what))
}
/// A pod template must not reach the node or gain privileges.
fn safe_pod(spec: &Value) -> Result<()> {
    let spec = object_map(spec, "Invalid pod template")?;
    for key in ["hostNetwork", "hostPID", "hostIPC"] {
        if spec.get(key).is_some_and(|v| v != &Value::Bool(false)) {
            return Err(invalid(
                "Workloads cannot use the host network or processes",
            ));
        }
    }
    if spec
        .get("serviceAccountName")
        .is_some_and(|v| v != "default")
        || spec.contains_key("nodeName")
        || spec.get("automountServiceAccountToken") == Some(&Value::Bool(true))
    {
        return Err(invalid("Workloads run without cluster credentials"));
    }
    for volume in spec
        .get("volumes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if volume.get("hostPath").is_some() {
            return Err(invalid("Workloads cannot mount host paths"));
        }
    }
    for list in ["containers", "initContainers", "ephemeralContainers"] {
        for container in spec
            .get(list)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let context = &container["securityContext"];
            let added = context["capabilities"]["add"]
                .as_array()
                .map_or(&[][..], Vec::as_slice);
            if context["privileged"] == true
                || context["allowPrivilegeEscalation"] == true
                || added.iter().any(|c| c != "NET_BIND_SERVICE")
            {
                return Err(invalid("Workloads cannot run privileged"));
            }
        }
    }
    Ok(())
}
use webdock_plugin_protocol::{MIB, MILLI, quantity};
/// Memory (limit) and CPU (request, or the limit Kubernetes would default it to) one container or
/// one CloudNativePG instance takes. Both must be stated.
fn resources(resources: &Value) -> Result<(u64, u64)> {
    let memory = quantity(&resources["limits"]["memory"], MIB)
        .ok_or_else(|| invalid("Every container states a memory limit"))?;
    let cpu = quantity(&resources["requests"]["cpu"], MILLI)
        .or_else(|| quantity(&resources["limits"]["cpu"], MILLI))
        .ok_or_else(|| invalid("Every container states a CPU request"))?;
    Ok((memory, cpu))
}
/// Restartable init containers keep running beside later init containers and the app.
fn pod(spec: &Value) -> Result<(u64, u64)> {
    let (mut memory, mut cpu, mut init_memory, mut init_cpu) = (0u64, 0u64, 0u64, 0u64);
    let (mut sidecar_memory, mut sidecar_cpu) = (0u64, 0u64);
    for container in spec["containers"].as_array().into_iter().flatten() {
        let (m, c) = resources(&container["resources"])?;
        memory = memory.saturating_add(m);
        cpu = cpu.saturating_add(c);
    }
    for container in spec["initContainers"].as_array().into_iter().flatten() {
        let (m, c) = resources(&container["resources"])?;
        init_memory = init_memory.max(sidecar_memory.saturating_add(m));
        init_cpu = init_cpu.max(sidecar_cpu.saturating_add(c));
        if container["restartPolicy"] == "Always" {
            sidecar_memory = sidecar_memory.saturating_add(m);
            sidecar_cpu = sidecar_cpu.saturating_add(c);
        }
    }
    Ok((
        memory.saturating_add(sidecar_memory).max(init_memory),
        cpu.saturating_add(sidecar_cpu).max(init_cpu),
    ))
}
fn storage(value: &Value) -> Result<u64> {
    quantity(value, MIB).ok_or_else(|| invalid("Every volume states its size"))
}
/// What one unit's objects take from the customer's plan: memory and storage in MiB, CPU in
/// millicores. Stopped workloads keep their storage.
pub fn allocation(objects: &[Value]) -> Result<Value> {
    let (mut memory, mut cpu, mut disk) = (0u64, 0u64, 0u64);
    for object in objects {
        let spec = &object["spec"];
        let count = |key: &str| spec[key].as_u64().unwrap_or(1);
        match object["kind"].as_str().unwrap_or_default() {
            kind @ ("Deployment" | "StatefulSet") => {
                let replicas = count("replicas");
                let (m, c) = pod(&spec["template"]["spec"])?;
                memory = memory.saturating_add(m.saturating_mul(replicas));
                cpu = cpu.saturating_add(c.saturating_mul(replicas));
                if kind == "StatefulSet" {
                    for claim in spec["volumeClaimTemplates"]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        let size = storage(&claim["spec"]["resources"]["requests"]["storage"])?;
                        disk = disk.saturating_add(size.saturating_mul(replicas.max(1)));
                    }
                }
            }
            "PersistentVolumeClaim" => {
                disk = disk.saturating_add(storage(&spec["resources"]["requests"]["storage"])?);
            }
            "Cluster" if object["apiVersion"] == "postgresql.cnpg.io/v1" => {
                let instances = count("instances");
                let (m, c) = resources(&spec["resources"])?;
                let mut size = storage(&spec["storage"]["size"])?;
                if !spec["walStorage"].is_null() {
                    size = size.saturating_add(storage(&spec["walStorage"]["size"])?);
                }
                memory = memory.saturating_add(m.saturating_mul(instances));
                cpu = cpu.saturating_add(c.saturating_mul(instances));
                disk = disk.saturating_add(size.saturating_mul(instances));
            }
            _ => {}
        }
    }
    Ok(json!({"memory": memory, "cpu": cpu, "storage": disk}))
}
/// A host a route or certificate may name: a lower-case DNS name of at least two labels, no wildcard.
fn host_name(value: &str) -> Option<String> {
    let host = value.to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    (host.len() <= 253 && labels.len() >= 2 && labels.iter().all(|l| dns_label(l))).then_some(host)
}
/// The hosts of one Traefik rule. The rule must start with its host matchers: one `Host(`a`)`, or
/// `Host(`a`) || Host(`b`)` alone or in parentheses, optionally followed by `&& <narrowing>` without
/// `||` or further host matchers. Anything else could match hosts the unit does not own.
fn rule_hosts(rule: &str, matcher: &str) -> Result<Vec<String>> {
    let refuse = || invalid("Routes start with the hosts they serve");
    let rule = rule.trim();
    let (part, rest) = match rule.strip_prefix('(') {
        Some(inner) => {
            // Hosts hold no backticks or parentheses, so the group ends at the first "`))".
            let close = inner.find("`))").ok_or_else(refuse)? + 2;
            (&inner[..close], Some(inner[close + 1..].trim()))
        }
        None => match rule.find("&&") {
            Some(at) => (rule[..at].trim(), Some(rule[at..].trim())),
            None => (rule, None),
        },
    };
    if let Some(rest) = rest.filter(|r| !r.is_empty()) {
        let narrowing = rest.strip_prefix("&&").ok_or_else(refuse)?;
        if narrowing.contains("||") || narrowing.contains("Host") {
            return Err(refuse());
        }
    }
    let mut hosts = Vec::new();
    for alternative in part.split("||") {
        let inner = alternative
            .trim()
            .strip_prefix(matcher)
            .and_then(|m| m.strip_prefix("(`"))
            .and_then(|m| m.strip_suffix("`)"))
            .filter(|m| !m.contains('`'))
            .ok_or_else(refuse)?;
        hosts
            .push(host_name(inner).ok_or_else(|| invalid("Routes name hosts by their full name"))?);
    }
    Ok(hosts)
}
/// Every host the unit's routes answer for or its certificates name.
pub fn hosts(objects: &[Value]) -> Result<Vec<String>> {
    let mut hosts = Vec::new();
    let names = |value: &Value| -> Result<Vec<String>> {
        value
            .as_array()
            .into_iter()
            .flatten()
            .map(|h| {
                h.as_str()
                    .and_then(host_name)
                    .ok_or_else(|| invalid("Routes name hosts by their full name"))
            })
            .collect()
    };
    for object in objects {
        let spec = &object["spec"];
        match object["kind"].as_str().unwrap_or_default() {
            kind @ ("IngressRoute" | "IngressRouteTCP") => {
                let matcher = if kind == "IngressRoute" {
                    "Host"
                } else {
                    "HostSNI"
                };
                for route in spec["routes"].as_array().into_iter().flatten() {
                    let rule = route["match"]
                        .as_str()
                        .ok_or_else(|| invalid("Routes need a rule"))?;
                    hosts.extend(rule_hosts(rule, matcher)?);
                }
            }
            "Ingress" => {
                for rule in spec["rules"].as_array().into_iter().flatten() {
                    hosts.push(
                        rule["host"]
                            .as_str()
                            .and_then(host_name)
                            .ok_or_else(|| invalid("Routes name hosts by their full name"))?,
                    );
                }
                if spec["rules"].as_array().is_none_or(Vec::is_empty)
                    || !spec["defaultBackend"].is_null()
                {
                    return Err(invalid("Routes start with the hosts they serve"));
                }
                for tls in spec["tls"].as_array().into_iter().flatten() {
                    hosts.extend(names(&tls["hosts"])?);
                }
            }
            "Certificate" => {
                for key in ["ipAddresses", "uris", "emailAddresses", "otherNames"] {
                    if !spec[key].is_null() {
                        return Err(invalid("Certificates name hosts only"));
                    }
                }
                hosts.extend(names(&spec["dnsNames"])?);
                if let Some(common) = spec["commonName"].as_str() {
                    hosts.push(
                        host_name(common).ok_or_else(|| invalid("Certificates name hosts only"))?,
                    );
                }
            }
            _ => {}
        }
    }
    hosts.sort();
    hosts.dedup();
    if hosts.len() > 64 {
        return Err(invalid("A unit serves at most 64 hosts"));
    }
    Ok(hosts)
}
/// Whether `host` is `domain` or one of its subdomains.
pub fn under(host: &str, domain: &str) -> bool {
    host == domain || host.strip_suffix(domain).is_some_and(|p| p.ends_with('.'))
}
/// One unit's objects as Core stores them.
pub struct Pinned {
    pub namespace: String,
    /// With namespace and owner labels set and service account tokens switched off.
    pub objects: Vec<Value>,
    /// What they take from the customer's plan.
    pub allocation: Value,
    /// Every host they route or certify.
    pub hosts: Vec<String>,
}
/// Checks and pins one unit's objects.
pub fn pin(
    manifest: &Manifest,
    plugin: &str,
    customer: &str,
    project: Option<&str>,
    unit: &str,
    objects: &Value,
) -> Result<Pinned> {
    if !dns_label(unit) {
        return Err(invalid("Invalid workload unit"));
    }
    let namespace = match project {
        Some(project) => namespace(plugin, project)?,
        None => customer_namespace(plugin, customer)?,
    };
    let list = objects
        .as_array()
        .filter(|l| (1..=MAX_OBJECTS).contains(&l.len()))
        .ok_or_else(|| invalid("Send between 1 and 32 objects"))?;
    if serde_json::to_vec(objects)
        .map_err(|_| invalid("Invalid objects"))?
        .len()
        > MAX_BYTES
    {
        return Err(invalid("Workload objects are too large"));
    }
    let mut seen = std::collections::HashSet::new();
    let mut pinned = Vec::with_capacity(list.len());
    for object in list {
        let mut object = object.clone();
        let api = object["apiVersion"].as_str().unwrap_or_default().to_owned();
        let kind = object["kind"].as_str().unwrap_or_default().to_owned();
        if !WORKLOAD_KINDS.contains(&format!("{api}/{kind}").as_str())
            || !manifest.workload_kind(&api, &kind)
        {
            return Err(invalid("Workload kind is not declared"));
        }
        let metadata = object_map(&object["metadata"], "Objects need metadata")?;
        if metadata
            .keys()
            .any(|k| !["name", "labels", "annotations"].contains(&k.as_str()))
        {
            return Err(invalid("Objects set name, labels and annotations only"));
        }
        let name = metadata
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| dns_label(n))
            .ok_or_else(|| invalid("Invalid object name"))?
            .to_owned();
        if !seen.insert(format!("{kind}/{name}")) {
            return Err(invalid("Duplicate object"));
        }
        for key in ["labels", "annotations"] {
            if let Some(map) = metadata.get(key) {
                let map = object_map(map, "Invalid labels")?;
                if map.len() > 32
                    || map
                        .iter()
                        .any(|(k, v)| k.starts_with(OWNER) || k.len() > 128 || !v.is_string())
                {
                    return Err(invalid("Invalid labels or annotations"));
                }
            }
        }
        match kind.as_str() {
            "Deployment" | "StatefulSet" => safe_pod(&object["spec"]["template"]["spec"])?,
            "Certificate" if object["spec"]["isCA"] == true => {
                return Err(invalid("Certificates are for servers, not authorities"));
            }
            "Service"
                if object["spec"]["type"]
                    .as_str()
                    .is_some_and(|t| t != "ClusterIP") =>
            {
                return Err(invalid(
                    "Services stay inside the cluster; publish through a route",
                ));
            }
            _ => {}
        }
        let labels = object["metadata"]["labels"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut labels = labels;
        labels.insert(format!("{OWNER}managed-by"), json!("webdock"));
        labels.insert(format!("{OWNER}plugin"), json!(plugin));
        labels.insert(format!("{OWNER}customer-id"), json!(customer));
        if let Some(project) = project {
            labels.insert(format!("{OWNER}project-id"), json!(project));
        }
        labels.insert(format!("{OWNER}unit"), json!(unit));
        object["metadata"]["labels"] = Value::Object(labels.clone());
        object["metadata"]["namespace"] = json!(namespace);
        if matches!(kind.as_str(), "Deployment" | "StatefulSet") {
            // Pods carry the owner labels too, so the agent can find them for status.
            let template = &mut object["spec"]["template"];
            let mut pod_labels = template["metadata"]["labels"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            pod_labels.retain(|key, _| !key.starts_with(OWNER));
            pod_labels.extend(labels);
            template["metadata"]["labels"] = Value::Object(pod_labels);
            template["spec"]["automountServiceAccountToken"] = json!(false);
        }
        pinned.push(object);
    }
    Ok(Pinned {
        allocation: allocation(&pinned)?,
        hosts: hosts(&pinned)?,
        namespace,
        objects: pinned,
    })
}

#[derive(Deserialize)]
#[serde(try_from = "ApplyInput")]
struct Apply {
    project: Option<String>,
    customer: Option<String>,
    unit: String,
    objects: Value,
    /// The plugin resource this unit belongs to, so usage can show its name.
    resource: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyInput {
    #[serde(default, deserialize_with = "owner_field")]
    project: Option<String>,
    #[serde(default, deserialize_with = "owner_field")]
    customer: Option<String>,
    unit: String,
    objects: Value,
    #[serde(default)]
    resource: Option<String>,
}
fn owner_field<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}
impl TryFrom<ApplyInput> for Apply {
    type Error = &'static str;
    fn try_from(input: ApplyInput) -> std::result::Result<Self, Self::Error> {
        if input.project.is_some() == input.customer.is_some()
            || input
                .project
                .iter()
                .chain(input.customer.iter())
                .any(|v| !numeric_owner(v))
            || input.customer.is_some() && input.resource.is_some()
        {
            return Err("Specify exactly one project or customer owner");
        }
        Ok(Self {
            project: input.project,
            customer: input.customer,
            unit: input.unit,
            objects: input.objects,
            resource: input.resource,
        })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Unit {
    unit: String,
}
impl HostService {
    /// `apply` | `observe` | `remove` for the authenticated plugin runtime.
    pub async fn workloads(
        self: &Arc<Self>,
        peer: &PeerIdentity,
        plugin: &str,
        generation: u64,
        method: &str,
        input: Value,
    ) -> Result<Value> {
        self.runtime_authority(plugin.to_owned(), generation)?
            .authorize_peer(peer)
            .await?;
        let tx = self.controller().await?;
        fence(&tx, false).await?;
        let metadata = self.metadata(&tx, plugin, None).await?;
        let approved = metadata.configuration["approved_capabilities"]
            .as_array()
            .is_some_and(|c| c.contains(&json!("plugin.workloads")));
        if !approved || metadata.manifest.as_json()["workloads"].is_null() {
            return Err(unsupported("Workloads are not approved for this plugin"));
        }
        let binding = self
            .bindings
            .runtime(plugin)
            .await?
            .ok_or_else(|| unsupported("Plugin runtime is not configured"))?;
        let output = match method {
            "apply" => {
                let input: Apply =
                    serde_json::from_value(input).map_err(|_| invalid("Invalid workload"))?;
                if !dns_label(&input.unit) {
                    return Err(invalid("Invalid workload unit"));
                }
                // Serialize one unit before looking up its immutable owner and existing cluster.
                tx.execute_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
                    [format!("webdock-workload:{plugin}:{}", input.unit).into()],
                ))
                .await
                .map_err(db_error)?;
                let previous = tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
                    "SELECT customer_id,project_id,cluster_id,allocation FROM webdock_auth.plugin_workload WHERE plugin_id=$1 AND unit=$2 FOR UPDATE",
                    [plugin.into(),input.unit.clone().into()])).await.map_err(db_error)?;
                let checked: Value = tx
                    .query_one_raw(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        "SELECT webdock_rls.workload_owner($1,$2) AS value",
                        [input.project.clone().into(), input.customer.clone().into()],
                    ))
                    .await
                    .map_err(db_error)?
                    .ok_or_else(unavailable)?
                    .try_get("", "value")
                    .map_err(db_error)?;
                let customer = checked["customer"]
                    .as_str()
                    .ok_or_else(PluginError::denied)?;
                let (cluster, previous_allocation) = match previous {
                    Some(row) => {
                        if row.try_get::<String>("", "customer_id").map_err(db_error)? != customer
                            || row
                                .try_get::<Option<String>>("", "project_id")
                                .map_err(db_error)?
                                != input.project
                        {
                            return Err(conflict("This unit belongs to another owner"));
                        }
                        (
                            row.try_get::<String>("", "cluster_id").map_err(db_error)?,
                            row.try_get::<Value>("", "allocation").map_err(db_error)?,
                        )
                    }
                    None => (
                        checked["cluster"]
                            .as_str()
                            .map(str::to_owned)
                            .or_else(|| binding.workload_cluster.clone())
                            .ok_or_else(|| unsupported("No cluster is assigned to this plugin"))?,
                        json!({}),
                    ),
                };
                if input.project.is_none() {
                    let allowed: bool = tx
                        .query_one_raw(Statement::from_sql_and_values(
                            DbBackend::Postgres,
                            "SELECT webdock_rls.customer_workload_cluster($1,$2) AS allowed",
                            [customer.into(), cluster.clone().into()],
                        ))
                        .await
                        .map_err(db_error)?
                        .ok_or_else(unavailable)?
                        .try_get("", "allowed")
                        .map_err(db_error)?;
                    if !allowed {
                        return Err(PluginError::denied());
                    }
                }
                let Pinned {
                    namespace,
                    objects,
                    allocation,
                    hosts,
                } = pin(
                    &metadata.manifest,
                    plugin,
                    customer,
                    input.project.as_deref(),
                    &input.unit,
                    &input.objects,
                )?;
                // Hosts under the plugin's own route domains are its to hand out; anything else
                // must be a domain the project's customer verified.
                let foreign: Vec<&String> = hosts
                    .iter()
                    .filter(|h| !binding.route_domains.iter().any(|d| under(h, d)))
                    .collect();
                if !foreign.is_empty() {
                    let row = tx
                        .query_one_raw(Statement::from_sql_and_values(
                            DbBackend::Postgres,
                            "SELECT webdock_rls.customer_unverified_host($1,$2) AS host",
                            [
                                customer.into(),
                                foreign
                                    .iter()
                                    .map(|h| (*h).clone())
                                    .collect::<Vec<_>>()
                                    .into(),
                            ],
                        ))
                        .await
                        .map_err(db_error)?
                        .ok_or_else(unavailable)?;
                    if let Some(host) = row
                        .try_get::<Option<String>>("", "host")
                        .map_err(db_error)?
                    {
                        return Err(conflict(&format!(
                            "Verify the domain of {host} before using it."
                        )));
                    }
                }
                if input.resource.as_deref().is_some_and(|r| {
                    !(1..=128).contains(&r.len())
                        || !r
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
                }) {
                    return Err(invalid("Invalid resource"));
                }
                if let Some(message) =
                    super::limits::exceeded(&checked, &previous_allocation, &allocation)
                {
                    return Err(conflict(&message));
                }
                let row = tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
                    "INSERT INTO webdock_auth.plugin_workload(plugin_id,unit,project_id,cluster_id,namespace,objects,allocation,resource_id,hosts,customer_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) \
                     ON CONFLICT (plugin_id,unit) DO UPDATE SET objects=EXCLUDED.objects,allocation=EXCLUDED.allocation,resource_id=EXCLUDED.resource_id,hosts=EXCLUDED.hosts,removing=false,revision=webdock_auth.plugin_workload.revision+1,updated_at=now() \
                     WHERE webdock_auth.plugin_workload.project_id IS NOT DISTINCT FROM EXCLUDED.project_id AND webdock_auth.plugin_workload.customer_id=EXCLUDED.customer_id AND webdock_auth.plugin_workload.cluster_id=EXCLUDED.cluster_id RETURNING revision",
                    [plugin.into(), input.unit.into(), input.project.into(), cluster.into(), namespace.clone().into(), Value::Array(objects).into(), allocation.into(), input.resource.into(), hosts.into(),customer.into()])).await.map_err(db_error)?
                    .ok_or_else(|| conflict("This unit belongs to another owner"))?;
                json!({"revision": row.try_get::<i64>("", "revision").map_err(db_error)?, "namespace": namespace})
            }
            "observe" => {
                let input: Unit =
                    serde_json::from_value(input).map_err(|_| invalid("Invalid workload"))?;
                let row = tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
                    "SELECT jsonb_build_object('revision',revision,'appliedRevision',applied_revision,'removing',removing,'observed',observed,'error',error,'observedAt',observed_at,'namespace',namespace) AS value FROM webdock_auth.plugin_workload WHERE plugin_id=$1 AND unit=$2",
                    [plugin.into(), input.unit.into()])).await.map_err(db_error)?;
                match row {
                    Some(row) => row.try_get::<Value>("", "value").map_err(db_error)?,
                    None => json!({"removed": true}),
                }
            }
            "remove" => {
                let input: Unit =
                    serde_json::from_value(input).map_err(|_| invalid("Invalid workload"))?;
                tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
                    "UPDATE webdock_auth.plugin_workload SET removing=true,objects='[]'::jsonb,allocation='{}'::jsonb,hosts='{}',revision=revision+1,updated_at=now() WHERE plugin_id=$1 AND unit=$2 AND NOT removing",
                    [plugin.into(), input.unit.into()])).await.map_err(db_error)?;
                json!({})
            }
            _ => return Err(unsupported("Unknown workload method")),
        };
        tx.commit().await.map_err(db_error)?;
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn apply_accepts_exactly_one_numeric_owner() {
        for owner in [json!({"project":"123"}), json!({"customer":"456"})] {
            let mut input = json!({"unit":"unit-1","objects":[]});
            input
                .as_object_mut()
                .unwrap()
                .extend(owner.as_object().unwrap().clone());
            assert!(serde_json::from_value::<Apply>(input).is_ok(), "{owner}");
        }
        for owner in [
            json!({}),
            json!({"project":"123","customer":"456"}),
            json!({"customer":null}),
            json!({"project":""}),
            json!({"customer":"foreign"}),
            json!({"customer":456}),
            json!({"customer":"456","unknown":true}),
        ] {
            let mut input = json!({"unit":"unit-1","objects":[]});
            input
                .as_object_mut()
                .unwrap()
                .extend(owner.as_object().unwrap().clone());
            assert!(serde_json::from_value::<Apply>(input).is_err(), "{owner}");
        }
    }
    #[test]
    fn customer_namespaces_and_pod_labels_cannot_claim_a_project() {
        let objects = json!([{"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"service"},"spec":{"template":{"metadata":{"labels":{"webdock.dev/project-id":"999"}},"spec":{"containers":[{"name":"service","resources":{"limits":{"memory":"128Mi"},"requests":{"cpu":"50m"}}}]}}}}]);
        let customer = pin(&manifest(), "hosting", "101", None, "service", &objects).unwrap();
        assert_eq!(customer.namespace, "wd-hosting-c101");
        assert_ne!(customer.namespace, namespace("hosting", "101").unwrap());
        for labels in [
            &customer.objects[0]["metadata"]["labels"],
            &customer.objects[0]["spec"]["template"]["metadata"]["labels"],
        ] {
            assert_eq!(labels["webdock.dev/customer-id"], "101");
            assert!(labels.get("webdock.dev/project-id").is_none());
        }
        let long = customer_namespace(&"p".repeat(64), "12345678901234567890").unwrap();
        assert!(long.len() <= 63 && long.ends_with("-c12345678901234567890"));
    }
    fn manifest() -> Manifest {
        let mut v: Value = serde_json::from_str(include_str!(
            "../../plugin-protocol/tests/fixtures/project-notes.json"
        ))
        .unwrap();
        v["name"] = json!("hosting");
        v["capabilities"] = json!(["plugin.workloads"]);
        v["services"]["required"] = json!([]);
        v["jobs"] = json!([]);
        v["workloads"] = json!({"kinds": ["apps/v1/Deployment", "v1/Service", "v1/Secret", "cert-manager.io/v1/Certificate", "postgresql.cnpg.io/v1/Cluster"]});
        for action in v["actions"].as_array_mut().unwrap() {
            action["capabilities"] = json!([]);
        }
        Manifest::parse(&serde_json::to_vec(&v).unwrap()).unwrap()
    }
    fn deployment(spec: Value) -> Value {
        json!({"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"web","labels":{"app":"web"}},
               "spec":{"selector":{"matchLabels":{"app":"web"}},"template":{"metadata":{"labels":{"app":"web"}},"spec":spec}}})
    }
    #[test]
    fn objects_are_pinned_to_the_installation_namespace_with_owner_labels() {
        let objects = json!([deployment(json!({"containers":[{"name":"web","image":"nginx:stable-alpine","resources":sized()}]})),
                             {"apiVersion":"v1","kind":"Service","metadata":{"name":"web"},"spec":{"ports":[{"port":80}]}}]);
        let Pinned {
            namespace,
            objects: pinned,
            ..
        } = pin(
            &manifest(),
            "hosting",
            "101",
            Some("201"),
            "app-1",
            &objects,
        )
        .unwrap();
        assert_eq!(namespace, "wd-hosting-201");
        assert_eq!(pinned[0]["metadata"]["namespace"], "wd-hosting-201");
        assert_eq!(pinned[0]["metadata"]["labels"]["webdock.dev/unit"], "app-1");
        assert_eq!(pinned[0]["metadata"]["labels"]["app"], "web");
        assert_eq!(
            pinned[0]["spec"]["template"]["metadata"]["labels"]["webdock.dev/plugin"],
            "hosting"
        );
        assert_eq!(
            pinned[0]["spec"]["template"]["spec"]["automountServiceAccountToken"],
            false
        );
        assert_eq!(
            pinned[1]["metadata"]["labels"]["webdock.dev/project-id"],
            "201"
        );
    }
    fn sized() -> Value {
        json!({"limits":{"memory":"256Mi"},"requests":{"memory":"256Mi","cpu":"50m"}})
    }
    #[test]
    fn allocation_counts_sidecars_during_startup_and_steady_state() {
        let container = |memory: &str, cpu: &str, sidecar: bool| {
            let mut value = json!({"name":"fixture","resources":{"limits":{"memory":memory},"requests":{"cpu":cpu}}});
            if sidecar {
                value["restartPolicy"] = json!("Always");
            }
            value
        };
        for (init, memory, cpu) in [
            (vec![container("128Mi", "100m", true)], 384, 150),
            (
                vec![
                    container("128Mi", "100m", true),
                    container("512Mi", "1", false),
                    container("64Mi", "25m", true),
                ],
                640,
                1100,
            ),
            (
                vec![
                    container("512Mi", "1", false),
                    container("128Mi", "100m", true),
                ],
                512,
                1000,
            ),
        ] {
            let objects = json!([deployment(
                json!({"containers":[{"name":"app","resources":sized()}],"initContainers":init})
            )]);
            assert_eq!(
                allocation(objects.as_array().unwrap()).unwrap(),
                json!({"memory":memory,"cpu":cpu,"storage":0})
            );
        }
    }
    #[test]
    fn allocation_counts_replicas_instances_and_volumes() {
        let pods = json!({"replicas":2,"template":{"spec":{
            "initContainers":[{"name":"i","resources":{"limits":{"memory":"1Gi"},"requests":{"cpu":"1"}}}],
            "containers":[{"name":"a","resources":sized()},{"name":"b","resources":{"limits":{"memory":"128Mi","cpu":"200m"}}}]}}});
        let mut stopped = json!({"apiVersion":"apps/v1","kind":"Deployment","spec":pods.clone()});
        stopped["spec"]["replicas"] = json!(0);
        let mut set = json!({"apiVersion":"apps/v1","kind":"StatefulSet","spec":pods.clone()});
        set["spec"]["volumeClaimTemplates"] =
            json!([{"spec":{"resources":{"requests":{"storage":"2Gi"}}}}]);
        let cases = [
            // Init containers run before the others: 1 GiB and one core outweigh 384 MiB and 250m.
            (
                json!([{"apiVersion":"apps/v1","kind":"Deployment","spec":pods}]),
                json!({"memory":2048,"cpu":2000,"storage":0}),
            ),
            (json!([stopped]), json!({"memory":0,"cpu":0,"storage":0})),
            (
                json!([set]),
                json!({"memory":2048,"cpu":2000,"storage":4096}),
            ),
            (
                json!([{"apiVersion":"v1","kind":"PersistentVolumeClaim","spec":{"resources":{"requests":{"storage":"2Gi"}}}}]),
                json!({"memory":0,"cpu":0,"storage":2048}),
            ),
            (
                json!([{"apiVersion":"postgresql.cnpg.io/v1","kind":"Cluster","spec":{"instances":2,"storage":{"size":"1Gi"},"walStorage":{"size":"512Mi"},
                "resources":{"limits":{"memory":"512Mi"},"requests":{"memory":"512Mi","cpu":"100m"}}}}]),
                json!({"memory":1024,"cpu":200,"storage":3072}),
            ),
            (
                json!([{"apiVersion":"v1","kind":"Secret"},{"apiVersion":"v1","kind":"Service"}]),
                json!({"memory":0,"cpu":0,"storage":0}),
            ),
        ];
        for (objects, expected) in cases {
            assert_eq!(
                allocation(objects.as_array().unwrap()).unwrap(),
                expected,
                "{objects}"
            );
        }
    }
    #[test]
    fn workloads_state_memory_cpu_and_volume_sizes() {
        let cases = [
            deployment(json!({"containers":[{"name":"a","image":"x"}]})),
            deployment(
                json!({"containers":[{"name":"a","image":"x","resources":{"limits":{"memory":"256Mi"}}}]}),
            ),
            deployment(
                json!({"containers":[{"name":"a","image":"x","resources":{"requests":{"cpu":"50m","memory":"256Mi"}}}]}),
            ),
            deployment(
                json!({"initContainers":[{"name":"i","image":"x"}],"containers":[{"name":"a","image":"x","resources":sized()}]}),
            ),
            json!({"apiVersion":"postgresql.cnpg.io/v1","kind":"Cluster","metadata":{"name":"db"},"spec":{"instances":1,"resources":{"limits":{"memory":"512Mi"},"requests":{"cpu":"100m"}}}}),
        ];
        for object in cases {
            assert!(
                pin(
                    &manifest(),
                    "hosting",
                    "101",
                    Some("201"),
                    "app-1",
                    &json!([object])
                )
                .is_err(),
                "{object}"
            );
        }
    }
    #[test]
    fn rules_start_with_the_hosts_they_serve() {
        let ok = [
            ("Host(`a.apps.localhost`)", vec!["a.apps.localhost"]),
            (
                "Host(`A.Shop.Example`) && PathPrefix(`/api`)",
                vec!["a.shop.example"],
            ),
            (
                "Host(`a.example`) || Host(`b.example`)",
                vec!["a.example", "b.example"],
            ),
            (
                "(Host(`a.example`) || Host(`b.example`) || Host(`c.example`)) && PathPrefix(`/`)",
                vec!["a.example", "b.example", "c.example"],
            ),
        ];
        for (rule, hosts) in ok {
            assert_eq!(rule_hosts(rule, "Host").unwrap(), hosts, "{rule}");
        }
        for rule in [
            "PathPrefix(`/`)",
            "PathPrefix(`/`) && Host(`a.example`)",
            "Host(`a.example`) || PathPrefix(`/`)",
            "Host(`a.example`) && Path(`/`) || Host(`b.example`)",
            "Host(`a.example`) && (Path(`/a`) || Path(`/b`))",
            "HostRegexp(`.+`)",
            "Host(`*.example`)",
            "Host(`localhost`)",
            "Host(`a.example`,`b.example`)",
            "!Host(`a.example`)",
            "(Host(`a.example`) || PathPrefix(`/`))",
        ] {
            assert!(rule_hosts(rule, "Host").is_err(), "{rule}");
        }
        assert_eq!(
            rule_hosts("HostSNI(`pg-1.db.localhost`)", "HostSNI").unwrap(),
            ["pg-1.db.localhost"]
        );
        assert!(rule_hosts("HostSNI(`*`)", "HostSNI").is_err());
    }
    #[test]
    fn hosts_come_from_routes_ingresses_and_certificates() {
        let objects = json!([
            {"apiVersion":"traefik.io/v1alpha1","kind":"IngressRoute","spec":{"routes":[{"match":"Host(`b.example`)"},{"match":"Host(`a.example`)"}]}},
            {"apiVersion":"traefik.io/v1alpha1","kind":"IngressRouteTCP","spec":{"routes":[{"match":"HostSNI(`db.example`)"}]}},
            {"apiVersion":"networking.k8s.io/v1","kind":"Ingress","spec":{"rules":[{"host":"c.example"}],"tls":[{"hosts":["c.example"]}]}},
            {"apiVersion":"cert-manager.io/v1","kind":"Certificate","spec":{"dnsNames":["a.example","www.a.example"],"commonName":"a.example"}},
        ]);
        assert_eq!(
            hosts(objects.as_array().unwrap()).unwrap(),
            [
                "a.example",
                "b.example",
                "c.example",
                "db.example",
                "www.a.example"
            ]
        );
        for bad in [
            json!({"apiVersion":"networking.k8s.io/v1","kind":"Ingress","spec":{"defaultBackend":{"service":{"name":"x"}},"rules":[{"host":"c.example"}]}}),
            json!({"apiVersion":"networking.k8s.io/v1","kind":"Ingress","spec":{"rules":[{"http":{}}]}}),
            json!({"apiVersion":"cert-manager.io/v1","kind":"Certificate","spec":{"ipAddresses":["10.0.0.1"]}}),
            json!({"apiVersion":"traefik.io/v1alpha1","kind":"IngressRoute","spec":{"routes":[{"kind":"Rule"}]}}),
        ] {
            assert!(hosts(std::slice::from_ref(&bad)).is_err(), "{bad}");
        }
        assert!(under("shop.example", "shop.example") && under("www.shop.example", "shop.example"));
        assert!(!under("evilshop.example", "shop.example") && !under("example", "shop.example"));
    }
    #[test]
    fn certificates_are_for_servers_only() {
        let certificate = |spec: Value| json!([{"apiVersion":"cert-manager.io/v1","kind":"Certificate","metadata":{"name":"web"},"spec":spec}]);
        let server = json!({"secretName":"web-tls","dnsNames":["web.apps.localhost"],"issuerRef":{"name":"webdock-dev-ca","kind":"ClusterIssuer"}});
        assert!(
            pin(
                &manifest(),
                "hosting",
                "101",
                Some("201"),
                "app-1",
                &certificate(server.clone())
            )
            .is_ok()
        );
        let mut authority = server;
        authority["isCA"] = json!(true);
        assert!(
            pin(
                &manifest(),
                "hosting",
                "101",
                Some("201"),
                "app-1",
                &certificate(authority)
            )
            .is_err(),
            "no plugin mints a CA"
        );
    }
    #[test]
    fn unsafe_or_undeclared_objects_are_refused() {
        let cases = [
            json!([{"apiVersion":"v1","kind":"Pod","metadata":{"name":"x"}}]),
            json!([{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"x"}}]),
            json!([{"apiVersion":"v1","kind":"Secret","metadata":{"name":"x","namespace":"kube-system"}}]),
            json!([{"apiVersion":"v1","kind":"Secret","metadata":{"name":"x","labels":{"webdock.dev/unit":"other"}}}]),
            json!([{"apiVersion":"v1","kind":"Secret","metadata":{"name":"Bad_Name"}}]),
            json!([{"apiVersion":"v1","kind":"Service","metadata":{"name":"x"},"spec":{"type":"NodePort"}}]),
            json!([deployment(
                json!({"hostNetwork":true,"containers":[{"name":"a","image":"x"}]})
            )]),
            json!([deployment(
                json!({"volumes":[{"name":"h","hostPath":{"path":"/"}}],"containers":[{"name":"a","image":"x"}]})
            )]),
            json!([deployment(
                json!({"containers":[{"name":"a","image":"x","securityContext":{"privileged":true}}]})
            )]),
            json!([deployment(
                json!({"containers":[{"name":"a","image":"x","securityContext":{"capabilities":{"add":["SYS_ADMIN"]}}}]})
            )]),
            json!([deployment(
                json!({"serviceAccountName":"admin","containers":[{"name":"a","image":"x"}]})
            )]),
            json!([{"apiVersion":"v1","kind":"Secret","metadata":{"name":"x"}},{"apiVersion":"v1","kind":"Secret","metadata":{"name":"x"}}]),
            json!([]),
        ];
        for objects in cases {
            assert!(
                pin(
                    &manifest(),
                    "hosting",
                    "101",
                    Some("201"),
                    "app-1",
                    &objects
                )
                .is_err(),
                "{objects}"
            );
        }
        assert!(
            pin(
                &manifest(),
                "hosting",
                "101",
                Some("201"),
                "App_1",
                &json!([{"apiVersion":"v1","kind":"Secret","metadata":{"name":"x"}}])
            )
            .is_err()
        );
        assert!(
            pin(
                &manifest(),
                "hosting",
                "101",
                Some("20x"),
                "app-1",
                &json!([{"apiVersion":"v1","kind":"Secret","metadata":{"name":"x"}}])
            )
            .is_err()
        );
        let long = namespace(&"p".repeat(64), "123456789012345678").unwrap();
        assert!(
            long.len() <= 63 && long.starts_with("wd-") && long.ends_with("-123456789012345678"),
            "{long}"
        );
    }
}
