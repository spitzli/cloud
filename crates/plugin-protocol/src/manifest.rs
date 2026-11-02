use crate::{
    CAPABILITIES, ErrorCode, MANIFEST_BYTES, PluginError, READ_CAPABILITIES, Result, WORKLOAD_KINDS,
};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use std::{collections::HashSet, fmt};
const NAME: &str = r"^[a-z][a-z0-9_.-]*$";
const FIELD: &str = r"^[a-z][a-zA-Z0-9_.-]*$";
const SLUG: &str = r"^[a-z][a-z0-9-]*$";
const SERVICES: &[&str] = &[
    "apps",
    "project",
    "data",
    "db",
    "cache",
    "jobs",
    "events",
    "storage",
    "secrets",
    "http",
    "audit",
    "log",
    "native-mail-control",
];
// Product names (mail, hosting, databases, ...) are plugins now; only platform kernel names stay reserved.
const RESERVED: &[&str] = &["core", "auth", "registry", "plugins", "billing"];
fn invalid() -> PluginError {
    PluginError::new(ErrorCode::InvalidInput, "Invalid plugin manifest or value")
}
fn ensure(ok: bool) -> Result<()> {
    if ok { Ok(()) } else { Err(invalid()) }
}
fn object<'a>(
    v: &'a Value,
    required: &[&str],
    optional: &[&str],
) -> Result<&'a Map<String, Value>> {
    let o = v.as_object().ok_or_else(invalid)?;
    ensure(
        required.iter().all(|k| o.contains_key(*k))
            && o.keys()
                .all(|k| required.contains(&k.as_str()) || optional.contains(&k.as_str())),
    )?;
    Ok(o)
}
fn text(v: &Value, min: usize, max: usize, pattern: Option<&str>) -> Result<()> {
    let s = v.as_str().ok_or_else(invalid)?;
    ensure(
        (min..=max).contains(&s.encode_utf16().count())
            && pattern.is_none_or(|p| regex::Regex::new(p).unwrap().is_match(s)),
    )
}
fn integer(v: &Value, min: i64, max: i64) -> Result<()> {
    ensure(v.as_i64().is_some_and(|n| n >= min && n <= max))
}
fn array(v: &Value, min: usize, max: usize) -> Result<&Vec<Value>> {
    let a = v.as_array().ok_or_else(invalid)?;
    ensure((min..=max).contains(&a.len()))?;
    Ok(a)
}
fn unique(a: &[Value], key: Option<&str>) -> Result<()> {
    let mut seen = HashSet::new();
    ensure(
        a.iter()
            .all(|v| seen.insert(key.map_or(v, |k| &v[k]).to_string())),
    )
}
fn names(v: &Value, min: usize, max: usize, choices: Option<&[&str]>) -> Result<()> {
    let a = array(v, min, max)?;
    unique(a, None)?;
    for v in a {
        text(v, 1, 64, Some(NAME))?;
        if let Some(c) = choices {
            ensure(c.contains(&v.as_str().unwrap()))?;
        }
    }
    Ok(())
}
fn exact_version(v: &Value) -> Result<()> {
    text(v, 1, 40, None)?;
    let s = v.as_str().unwrap();
    let version: node_semver::Version = s.parse().map_err(|_| invalid())?;
    ensure(version.to_string() == s)
}
fn literal(v: &Value) -> Result<()> {
    ensure(
        v.is_null()
            || v.is_boolean()
            || v.is_number()
            || v.as_str().is_some_and(|s| s.encode_utf16().count() <= 4096),
    )
}
fn type_matches(v: &Value, kind: &str) -> bool {
    match kind {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => v
            .as_i64()
            .is_some_and(|n| n.abs_diff(0) <= 9_007_199_254_740_991),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => false,
    }
}
fn field_name(value: &Value) -> Result<()> {
    text(value, 1, 64, Some(FIELD))?;
    ensure(
        !value
            .as_str()
            .unwrap()
            .split(['.', '-'])
            .any(|p| ["__proto__", "constructor", "prototype"].contains(&p)),
    )
}
fn schema(v: &Value, depth: usize) -> Result<()> {
    ensure(depth <= 3)?;
    let o = object(
        v,
        &["type"],
        &[
            "properties",
            "required",
            "additionalProperties",
            "items",
            "enum",
            "default",
            "minLength",
            "maxLength",
            "minimum",
            "maximum",
            "minItems",
            "maxItems",
            "description",
        ],
    )?;
    let kind = v["type"].as_str().ok_or_else(invalid)?;
    ensure(
        [
            "object", "array", "string", "number", "integer", "boolean", "null",
        ]
        .contains(&kind),
    )?;
    if let Some(props) = o.get("properties") {
        let props = props.as_object().ok_or_else(invalid)?;
        ensure(props.len() <= 64)?;
        for (key, value) in props {
            field_name(&Value::String(key.clone()))?;
            schema(value, depth + 1)?;
        }
    }
    if let Some(required) = o.get("required") {
        let required = array(required, 0, 64)?;
        unique(required, None)?;
        for key in required {
            field_name(key)?;
        }
        for key in required {
            ensure(v["properties"].get(key.as_str().unwrap()).is_some())?;
        }
    }
    if let Some(additional) = o.get("additionalProperties") {
        ensure(additional == false)?;
    }
    if let Some(items) = o.get("items") {
        schema(items, depth + 1)?;
    } else {
        ensure(kind != "array")?;
    }
    for (min, max, bound) in [
        ("minLength", "maxLength", 65536),
        ("minItems", "maxItems", 100),
        ("minimum", "maximum", -1),
    ] {
        for key in [min, max] {
            if let Some(n) = o.get(key) {
                if bound >= 0 {
                    integer(n, 0, bound)?;
                } else {
                    ensure(n.is_number())?;
                }
            }
        }
        if let (Some(a), Some(b)) = (o.get(min), o.get(max)) {
            ensure(a.as_f64().unwrap() <= b.as_f64().unwrap())?;
        }
    }
    if let Some(d) = o.get("description") {
        text(d, 0, 2000, None)?;
    }
    if let Some(d) = o.get("default") {
        literal(d)?;
        ensure(type_matches(d, kind))?;
    }
    if let Some(items) = o.get("enum") {
        for item in array(items, 1, 64)? {
            literal(item)?;
            ensure(type_matches(item, kind))?;
        }
    }
    Ok(())
}
fn column(v: &Value) -> Result<()> {
    let o = object(v, &["name", "type", "nullable"], &["default"])?;
    text(&v["name"], 1, 64, Some(NAME))?;
    ensure(v["nullable"].is_boolean())?;
    let kind = v["type"].as_str().ok_or_else(invalid)?;
    ensure(
        [
            "string",
            "integer",
            "boolean",
            "timestamp",
            "json",
            "decimal",
        ]
        .contains(&kind),
    )?;
    if let Some(d) = o.get("default") {
        literal(d)?;
        let ok = if d.is_null() {
            v["nullable"] == true
        } else {
            match kind {
                "timestamp" => d
                    .as_str()
                    .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok()),
                "decimal" => d.as_str().is_some_and(|s| {
                    regex::Regex::new(r"^-?(0|[1-9][0-9]*)(\.[0-9]+)?$")
                        .unwrap()
                        .is_match(s)
                }),
                "json" => true,
                _ => type_matches(d, kind),
            }
        };
        ensure(ok)?;
    }
    Ok(())
}
fn index(v: &Value) -> Result<()> {
    object(v, &["name", "columns", "unique"], &[])?;
    text(&v["name"], 1, 64, Some(NAME))?;
    names(&v["columns"], 1, 16, None)?;
    ensure(v["unique"].is_boolean())
}
fn foreign_key(v: &Value) -> Result<()> {
    object(v, &["name", "columns", "references"], &[])?;
    text(&v["name"], 1, 64, Some(NAME))?;
    names(&v["columns"], 1, 16, None)?;
    object(&v["references"], &["table", "columns"], &[])?;
    text(&v["references"]["table"], 1, 64, Some(NAME))?;
    names(&v["references"]["columns"], 1, 16, None)
}
fn table(v: &Value) -> Result<()> {
    object(
        v,
        &["name", "columns", "primaryKey", "indexes", "foreignKeys"],
        &[],
    )?;
    text(&v["name"], 1, 64, Some(NAME))?;
    let columns = array(&v["columns"], 1, 64)?;
    unique(columns, Some("name"))?;
    for c in columns {
        column(c)?;
    }
    names(&v["primaryKey"], 1, 16, None)?;
    let indexes = array(&v["indexes"], 0, 32)?;
    unique(indexes, Some("name"))?;
    for i in indexes {
        index(i)?;
    }
    let fks = array(&v["foreignKeys"], 0, 32)?;
    unique(fks, Some("name"))?;
    for fk in fks {
        foreign_key(fk)?;
        ensure(
            fk["columns"].as_array().unwrap().len()
                == fk["references"]["columns"].as_array().unwrap().len(),
        )?;
    }
    for key in std::iter::once(&v["primaryKey"])
        .chain(indexes.iter().map(|i| &i["columns"]))
        .chain(fks.iter().map(|i| &i["columns"]))
    {
        for key in key.as_array().unwrap() {
            ensure(columns.iter().any(|c| c["name"] == *key))?;
        }
    }
    for key in v["primaryKey"].as_array().unwrap() {
        ensure(
            columns
                .iter()
                .any(|c| c["name"] == *key && c["nullable"] == false),
        )?;
    }
    Ok(())
}

/// Validated v2 descriptor. Retains exact field values; signatures must use original bytes.
#[derive(Debug, Clone)]
pub struct Manifest {
    value: Value,
    bytes: Vec<u8>,
}
impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure(bytes.len() <= MANIFEST_BYTES)?;
        let value: StrictJson = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        check_depth(&value.0, 0)?;
        validate_manifest(&value.0)?;
        Ok(Self {
            value: value.0,
            bytes: bytes.to_vec(),
        })
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn require_host_services(&self, available: &[(&str, u32)]) -> Result<()> {
        for service in self.value["services"]["required"].as_array().unwrap() {
            let name = service["name"].as_str().unwrap();
            if !available.contains(&(name, 1)) {
                return Err(PluginError::new(
                    ErrorCode::CapabilityUnavailable,
                    format!("Required host service {name} v1 unavailable"),
                ));
            }
        }
        Ok(())
    }
    pub fn as_json(&self) -> &Value {
        &self.value
    }
    pub fn id(&self) -> &str {
        self.value["name"].as_str().unwrap()
    }
    pub fn version(&self) -> &str {
        self.value["version"].as_str().unwrap()
    }
    pub fn action(&self, name: &str) -> Option<&Value> {
        self.value["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == name)
    }
    pub fn job(&self, name: &str) -> Option<&Value> {
        find(&self.value["jobs"], "name", name)
    }
    pub fn offering(&self, id: &str) -> Option<&Value> {
        find(&self.value["offerings"], "id", id)
    }
    pub fn feature(&self, id: &str) -> Option<&Value> {
        find(&self.value["selfService"]["features"], "id", id)
    }
    pub fn features(&self) -> &[Value] {
        self.value["selfService"]["features"].as_array().unwrap()
    }
    pub fn operator_pages(&self) -> &[Value] {
        self.value["areas"]["operator"]["pages"].as_array().unwrap()
    }
    /// Whether the manifest declares this Kubernetes kind for its workloads.
    pub fn workload_kind(&self, api_version: &str, kind: &str) -> bool {
        let wanted = format!("{api_version}/{kind}");
        self.value["workloads"]["kinds"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|k| k == wanted.as_str()))
    }
    pub fn self_service_pages(&self) -> &[Value] {
        self.value["areas"]["selfService"]["pages"]
            .as_array()
            .map_or(&[], Vec::as_slice)
    }
    pub fn require_action_only_lifecycle(&self) -> Result<()> {
        if ["dependencies", "tables", "migrations"]
            .iter()
            .any(|k| !self.value[k].as_array().unwrap().is_empty())
            || self.value["jobs"].as_array().unwrap().iter().any(|job| {
                job.get("intervalSeconds").is_some()
                    || job["capabilities"].as_array().unwrap().iter().any(|cap| {
                        ![
                            "plugin.data.read",
                            "plugin.data.write",
                            "plugin.http.read",
                            "plugin.http.request",
                        ]
                        .contains(&cap.as_str().unwrap())
                    })
            })
        {
            return Err(PluginError::new(
                ErrorCode::NotSupported,
                "Installer supports actions and immediate data/HTTP jobs only",
            ));
        }
        Ok(())
    }
}

fn validate_manifest(v: &Value) -> Result<()> {
    let o = object(
        v,
        &[
            "name",
            "version",
            "kind",
            "protocolVersion",
            "sdkVersion",
            "dataProtocolVersion",
            "coreVersion",
            "image",
            "dependencies",
            "capabilities",
            "schemaVersion",
            "readableSchema",
            "migrations",
            "tables",
            "services",
            "configuration",
            "actions",
            "jobs",
            "offerings",
            "selfService",
            "areas",
            "events",
            "secrets",
            "httpBindings",
        ],
        &[
            "description",
            "category",
            "tags",
            "license",
            "icon",
            "screenshots",
            "readme",
            "readmes",
            "documentation_url",
            "homepage",
            "support",
            "min_runtime_version",
            "api",
            "mcp",
            "workloads",
        ],
    )?;
    // Workloads: a declared, bounded set of namespaced kinds, together with the capability.
    let declares = v["capabilities"]
        .as_array()
        .is_some_and(|c| c.contains(&Value::from("plugin.workloads")));
    match o.get("workloads") {
        Some(w) => {
            ensure(declares)?;
            object(w, &["kinds"], &[])?;
            let kinds = array(&w["kinds"], 1, WORKLOAD_KINDS.len())?;
            unique(kinds, None)?;
            for kind in kinds {
                ensure(kind.as_str().is_some_and(|k| WORKLOAD_KINDS.contains(&k)))?;
            }
        }
        None => ensure(!declares)?,
    }
    text(&v["name"], 1, 64, Some(SLUG))?;
    ensure(!RESERVED.contains(&v["name"].as_str().unwrap()))?;
    exact_version(&v["version"])?;
    ensure(v["kind"] == "extension")?;
    for k in ["protocolVersion", "sdkVersion", "dataProtocolVersion"] {
        ensure(v[k] == 2)?;
    }
    text(&v["coreVersion"], 1, 120, None)?;
    v["coreVersion"]
        .as_str()
        .unwrap()
        .parse::<node_semver::Range>()
        .map_err(|_| invalid())?;
    text(
        &v["image"],
        1,
        500,
        Some(
            r"^[a-z0-9]+(?:[.-][a-z0-9]+)*(?::[0-9]{1,5})?/(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*@sha256:[a-f0-9]{64}$",
        ),
    )?;
    let deps = array(&v["dependencies"], 0, 32)?;
    unique(deps, Some("id"))?;
    for d in deps {
        object(d, &["id", "version"], &[])?;
        text(&d["id"], 1, 64, Some(SLUG))?;
        exact_version(&d["version"])?;
        ensure(d["id"] != v["name"])?;
    }
    names(&v["capabilities"], 0, 32, Some(CAPABILITIES))?;
    names(&v["secrets"], 0, 32, None)?;
    names(&v["httpBindings"], 0, 32, None)?;
    integer(&v["schemaVersion"], 1, i64::from(i32::MAX))?;
    object(&v["readableSchema"], &["min", "max"], &[])?;
    for k in ["min", "max"] {
        integer(&v["readableSchema"][k], 1, i64::from(i32::MAX))?;
    }
    ensure(
        v["readableSchema"]["min"].as_i64() <= v["schemaVersion"].as_i64()
            && v["readableSchema"]["max"].as_i64() >= v["schemaVersion"].as_i64(),
    )?;
    let tables = array(&v["tables"], 0, 32)?;
    unique(tables, Some("name"))?;
    for t in tables {
        table(t)?;
    }
    let migrations = array(&v["migrations"], 0, 256)?;
    for (i, m) in migrations.iter().enumerate() {
        object(m, &["version", "checksum", "steps"], &[])?;
        ensure(m["version"] == i + 1 && m["version"].as_i64() <= v["schemaVersion"].as_i64())?;
        text(&m["checksum"], 64, 64, Some(r"^[a-f0-9]{64}$"))?;
        for step in array(&m["steps"], 1, 64)? {
            match step["op"].as_str() {
                Some("createTable") => {
                    object(step, &["op", "table"], &[])?;
                    table(&step["table"])?;
                }
                Some("addNullableColumn") => {
                    object(step, &["op", "table", "column"], &[])?;
                    text(&step["table"], 1, 64, Some(NAME))?;
                    column(&step["column"])?;
                    ensure(step["column"]["nullable"] == true)?;
                }
                Some("createIndex") => {
                    object(step, &["op", "table", "index"], &[])?;
                    text(&step["table"], 1, 64, Some(NAME))?;
                    index(&step["index"])?;
                }
                Some("addForeignKey") => {
                    object(step, &["op", "table", "foreignKey"], &[])?;
                    text(&step["table"], 1, 64, Some(NAME))?;
                    foreign_key(&step["foreignKey"])?;
                }
                _ => return Err(invalid()),
            }
        }
    }
    ensure(tables.is_empty() || migrations.len() as i64 == v["schemaVersion"].as_i64().unwrap())?;
    for t in tables {
        for fk in t["foreignKeys"].as_array().unwrap() {
            let target = tables
                .iter()
                .find(|t| t["name"] == fk["references"]["table"])
                .ok_or_else(invalid)?;
            ensure(
                target["primaryKey"] == fk["references"]["columns"]
                    || target["indexes"].as_array().unwrap().iter().any(|i| {
                        i["unique"] == true && i["columns"] == fk["references"]["columns"]
                    }),
            )?;
        }
    }
    object(&v["services"], &["required", "optional"], &[])?;
    let mut services = Vec::new();
    for k in ["required", "optional"] {
        for service in array(&v["services"][k], 0, 16)? {
            object(service, &["name", "version"], &[])?;
            ensure(
                service["name"]
                    .as_str()
                    .is_some_and(|s| SERVICES.contains(&s))
                    && service["version"] == 1,
            )?;
            services.push(service.clone());
        }
    }
    unique(&services, Some("name"))?;
    schema(&v["configuration"], 0)?;
    for (key, max) in [("actions", 64), ("jobs", 32)] {
        let entries = array(&v[key], 0, max)?;
        unique(entries, Some("name"))?;
        for entry in entries {
            let required = match key {
                "actions" => vec!["name", "intent", "audience", "capabilities"],
                _ => vec!["name", "capabilities"],
            };
            let optional = match key {
                "actions" => vec![
                    "input",
                    "output",
                    "storage",
                    "httpBinding",
                    "destructive",
                    "offering",
                    "feature",
                    "resource",
                ],
                _ => vec!["intervalSeconds", "input"],
            };
            let e = object(entry, &required, &optional)?;
            names(&entry["capabilities"], 0, 32, Some(CAPABILITIES))?;
            for c in entry["capabilities"].as_array().unwrap() {
                ensure(v["capabilities"].as_array().unwrap().contains(c))?;
            }
            text(&entry["name"], 1, 64, Some(NAME))?;
            for k in ["input", "output"] {
                if let Some(s) = e.get(k) {
                    schema(s, 0)?;
                }
            }
            if key == "jobs" {
                if let Some(i) = e.get("intervalSeconds") {
                    integer(i, 60, i64::from(i32::MAX))?;
                }
                continue;
            }
            ensure(entry["intent"] == "read" || entry["intent"] == "write")?;
            if entry["intent"] == "read" {
                for c in entry["capabilities"].as_array().unwrap() {
                    ensure(READ_CAPABILITIES.contains(&c.as_str().unwrap()))?;
                }
            }
            if let Some(d) = e.get("destructive") {
                ensure(d.is_boolean())?;
            }
            if let Some(s) = e.get("storage") {
                ensure(
                    (s == "upload"
                        && entry["intent"] == "write"
                        && entry["capabilities"]
                            .as_array()
                            .unwrap()
                            .contains(&Value::from("plugin.storage.write")))
                        || (s == "download"
                            && entry["capabilities"]
                                .as_array()
                                .unwrap()
                                .contains(&Value::from("plugin.storage.read"))),
                )?;
            }
            if let Some(binding) = e.get("httpBinding") {
                text(binding, 1, 64, Some(NAME))?;
                let caps = entry["capabilities"].as_array().unwrap();
                ensure(
                    v["httpBindings"].as_array().unwrap().contains(binding)
                        && !caps.is_empty()
                        && caps
                            .iter()
                            .all(|c| c == "plugin.http.read" || c == "plugin.http.request")
                        && caps.contains(&Value::from(if entry["intent"] == "write" {
                            "plugin.http.request"
                        } else {
                            "plugin.http.read"
                        })),
                )?;
            }
        }
    }
    self_service(v)?;
    let events = array(&v["events"], 0, 32)?;
    unique(events, Some("name"))?;
    for e in events {
        object(e, &["name", "version", "schema", "handlers"], &[])?;
        text(&e["name"], 1, 64, Some(NAME))?;
        integer(&e["version"], 1, i64::from(i32::MAX))?;
        schema(&e["schema"], 0)?;
        names(&e["handlers"], 1, 32, None)?;
        for h in e["handlers"].as_array().unwrap() {
            ensure(
                v["jobs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|j| j["name"] == *h),
            )?;
        }
    }
    for k in ["api", "mcp"] {
        if let Some(items) = o.get(k) {
            let items = array(items, 0, 64)?;
            let mut routes = HashSet::new();
            for item in items {
                if k == "api" {
                    object(item, &["method", "path", "action"], &[])?;
                    ensure(
                        item["method"].as_str().is_some_and(|m| {
                            ["GET", "POST", "PUT", "PATCH", "DELETE"].contains(&m)
                        }),
                    )?;
                    text(
                        &item["path"],
                        1,
                        160,
                        Some(r"^/(?:[a-z][a-z0-9-]*/)*[a-z][a-z0-9-]*$"),
                    )?;
                    ensure(routes.insert(format!("{} {}", item["method"], item["path"])))?;
                } else {
                    object(item, &["name", "description", "action"], &[])?;
                    text(&item["name"], 1, 64, Some(r"^[a-z][a-z0-9_]*$"))?;
                    text(&item["description"], 1, 1000, None)?;
                    ensure(routes.insert(item["name"].to_string()))?;
                }
                text(&item["action"], 1, 64, Some(NAME))?;
                let action = v["actions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|a| a["name"] == item["action"])
                    .ok_or_else(invalid)?;
                ensure(k != "api" || item["method"] != "GET" || action["intent"] == "read")?;
            }
        }
    }
    for (k, max) in [
        ("description", 280),
        ("category", 40),
        ("license", 40),
        ("icon", 500),
        ("readme", 500),
        ("min_runtime_version", 40),
    ] {
        if let Some(s) = o.get(k) {
            text(s, 0, max, None)?;
        }
    }
    if let Some(tags) = o.get("tags") {
        for tag in array(tags, 0, 16)? {
            text(tag, 0, 30, None)?;
        }
    }
    if let Some(screens) = o.get("screenshots") {
        for s in array(screens, 0, 12)? {
            let o = object(s, &["url"], &["caption", "alt"])?;
            text(&s["url"], 1, 500, None)?;
            for k in ["caption", "alt"] {
                if let Some(s) = o.get(k) {
                    text(s, 0, 200, None)?;
                }
            }
        }
    }
    if let Some(readmes) = o.get("readmes") {
        for (k, v) in readmes.as_object().ok_or_else(invalid)? {
            text(
                &Value::from(k.as_str()),
                1,
                5,
                Some(r"^[a-z]{2}(-[A-Z]{2})?$"),
            )?;
            text(v, 0, 500, None)?;
        }
    }
    for key in ["documentation_url", "homepage"] {
        if let Some(v) = o.get(key) {
            display_url(v)?;
        }
    }
    if let Some(support) = o.get("support") {
        let o = object(support, &[], &["email", "issues_url"])?;
        if let Some(email) = o.get("email") {
            text(email, 0, 254, None)?;
        }
        if let Some(url) = o.get("issues_url") {
            display_url(url)?;
        }
    }
    Ok(())
}
fn find<'a>(list: &'a Value, key: &str, id: &str) -> Option<&'a Value> {
    list.as_array()?.iter().find(|item| item[key] == id)
}
/// Offerings, self-service features, action audiences/gates and the two UI areas.
fn self_service(v: &Value) -> Result<()> {
    let labelled = |list: &Value, max: usize, optional: &[&str]| -> Result<()> {
        let items = array(list, 0, max)?;
        unique(items, Some("id"))?;
        for item in items {
            let o = object(item, &["id", "label"], optional)?;
            text(&item["id"], 1, 64, Some(SLUG))?;
            text(&item["label"], 1, 80, None)?;
            if let Some(d) = o.get("description") {
                text(d, 0, 280, None)?;
            }
        }
        Ok(())
    };
    // Offerings explain themselves to customers: what it is (description) and what it brings (highlights).
    labelled(&v["offerings"], 16, &["description", "highlights"])?;
    for o in v["offerings"].as_array().unwrap() {
        text(o.get("description").ok_or_else(invalid)?, 1, 280, None)?;
        if let Some(h) = o.get("highlights") {
            for item in array(h, 1, 6)? {
                text(item, 1, 140, None)?;
            }
        }
    }
    object(&v["selfService"], &["features"], &[])?;
    let features = &v["selfService"]["features"];
    labelled(features, 32, &["offering", "description"])?;
    for f in features.as_array().unwrap() {
        if let Some(o) = f.get("offering") {
            ensure(find(&v["offerings"], "id", o.as_str().ok_or_else(invalid)?).is_some())?;
        }
    }
    let areas = object(&v["areas"], &["operator"], &["selfService"])?;
    let actions = v["actions"].as_array().unwrap();
    for a in actions {
        let customer = match a["audience"].as_str() {
            Some("customer") => true,
            Some("operator") => false,
            _ => return Err(invalid()),
        };
        ensure(!customer || areas.contains_key("selfService"))?;
        let feature = a.get("feature");
        let resource = a
            .get("resource")
            .map(|r| r.as_str().ok_or_else(invalid))
            .transpose()?;
        if let Some(o) = a.get("offering") {
            ensure(find(&v["offerings"], "id", o.as_str().ok_or_else(invalid)?).is_some())?;
        }
        if let Some(f) = feature {
            ensure(customer && find(features, "id", f.as_str().ok_or_else(invalid)?).is_some())?;
        }
        ensure(customer == (feature.is_some() || resource.is_some()))?;
        // Creating and deleting change state: members and read-only tokens must never reach them.
        ensure(!matches!(resource, Some("create" | "delete")) || a["intent"] == "write")?;
        let id_input = &a["input"]["properties"]["resource_id"];
        match resource {
            None => {}
            Some("create") => {
                ensure(feature.is_some() && a.get("offering").is_some() && id_input.is_null())?
            }
            Some("use" | "delete") => ensure(
                id_input["type"] == "string"
                    && id_input["maxLength"].as_i64().is_some_and(|n| n <= 128)
                    && a["input"]["required"]
                        .as_array()
                        .is_some_and(|r| r.contains(&Value::from("resource_id"))),
            )?,
            _ => return Err(invalid()),
        }
    }
    for f in features.as_array().unwrap() {
        ensure(actions.iter().any(|a| a["feature"] == f["id"]))?;
    }
    for (area, customer) in [("operator", false), ("selfService", true)] {
        let Some(area) = areas.get(area) else {
            continue;
        };
        object(area, &["pages"], &[])?;
        let pages = array(&area["pages"], 0, 16)?;
        unique(pages, Some("id"))?;
        for page in pages {
            object(
                page,
                &["id", "label", "action"],
                if customer { &["resource"] } else { &[] },
            )?;
            text(&page["id"], 1, 64, Some(SLUG))?;
            text(&page["label"], 1, 80, None)?;
            let action = find(
                &v["actions"],
                "name",
                page["action"].as_str().ok_or_else(invalid)?,
            )
            .ok_or_else(invalid)?;
            ensure(action["intent"] == "read")?;
            // ponytail: self-service pages are resource pages; project-level lists come from Core.
            ensure(if customer {
                page["resource"] == true
                    && action["audience"] == "customer"
                    && action["resource"] == "use"
            } else {
                action["audience"] == "operator"
            })?;
        }
    }
    Ok(())
}
fn display_url(v: &Value) -> Result<()> {
    text(v, 0, 500, None)?;
    let url = url::Url::parse(v.as_str().unwrap()).map_err(|_| invalid())?;
    ensure(["http", "https"].contains(&url.scheme()))
}

/// Validate action/configuration values using local JSON Schema only.
pub fn validate_value(schema: Option<&Value>, value: &Value) -> Result<()> {
    let Some(s) = schema else {
        return Ok(());
    };
    // Apply the wire restriction even for callers that did not obtain the schema from Manifest.
    crate::manifest::schema(s, 0)?;
    let validator = jsonschema::validator_for(s).map_err(|_| invalid())?;
    ensure(validator.is_valid(value))
}
fn check_depth(v: &Value, depth: usize) -> Result<()> {
    ensure(depth <= 24)?;
    if let Some(a) = v.as_array() {
        for v in a {
            check_depth(v, depth + 1)?;
        }
    }
    if let Some(o) = v.as_object() {
        for (k, v) in o {
            ensure(
                !["__proto__", "constructor", "prototype", "$ref", "$id"].contains(&k.as_str()),
            )?;
            check_depth(v, depth + 1)?;
        }
    }
    Ok(())
}
// Reject duplicate object keys before converting to Value; a parser must not silently choose authority.
pub(crate) fn strict_json(bytes: &[u8]) -> Result<Value> {
    serde_json::from_slice::<StrictJson>(bytes)
        .map(|value| value.0)
        .map_err(|_| invalid())
}
struct StrictJson(Value);
impl<'de> Deserialize<'de> for StrictJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct JsonVisitor;
        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = StrictJson;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Self::Value, E> {
                if v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_991.0 {
                    return Ok(StrictJson(Value::from(v as i64)));
                }
                serde_json::Number::from_f64(v)
                    .map(|n| StrictJson(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite JSON"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut a = Vec::new();
                while let Some(v) = seq.next_element::<StrictJson>()? {
                    a.push(v.0);
                }
                Ok(StrictJson(Value::Array(a)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut o = Map::new();
                while let Some((k, v)) = map.next_entry::<String, StrictJson>()? {
                    if o.insert(k, v.0).is_some() {
                        return Err(de::Error::custom("duplicate JSON key"));
                    }
                }
                Ok(StrictJson(Value::Object(o)))
            }
        }
        deserializer.deserialize_any(JsonVisitor)
    }
}
