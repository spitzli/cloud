//! Existing database attachments. Only explicit URI reveal may return a credential.
use crate::{Connections, Error, Guard, Result, field, load, neon, one, purpose, revision};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use webdock_plugin_host::http::{HttpAdapterContext, HttpResult, NamedHttpAdapter};

pub(crate) fn inventory_path(path: &str) -> Result<(Option<&str>, &str)> {
    let parts: Vec<_> = path.split('/').collect();
    let (project, kind) = match parts.as_slice() {
        ["", "projects"] => (None, "projects"),
        ["", "projects", project] if neon::identifier(project) => (Some(*project), "project"),
        ["", "projects", project, kind @ ("branches" | "endpoints")]
            if neon::identifier(project) =>
        {
            (Some(*project), *kind)
        }
        [
            "",
            "projects",
            project,
            "branches",
            branch,
            kind @ ("databases" | "roles"),
        ] if neon::identifier(project) && neon::identifier(branch) => (Some(*project), *kind),
        ["", "projects", project, "branches", branch]
            if neon::identifier(project) && neon::identifier(branch) =>
        {
            (Some(*project), "branch")
        }
        ["", "projects", project, "endpoints", endpoint]
            if neon::identifier(project) && neon::identifier(endpoint) =>
        {
            (Some(*project), "endpoint")
        }
        _ => return Err(Error::InvalidInput),
    };
    Ok((project, kind))
}
fn safe_text(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        !text.is_empty() && text.len() <= 512 && !text.chars().any(char::is_control)
    })
}
/// Positive field projection, including on roles: no recursive provider objects are forwarded.
pub(crate) fn project_inventory(path: &str, raw: Value) -> Result<Value> {
    let (_, kind) = inventory_path(path)?;
    let fields: &[&str] = match kind {
        "projects" | "project" => &["id", "name", "region_id"],
        "branches" | "branch" => &["id", "name", "project_id", "parent_id", "current_state"],
        "databases" => &["id", "name", "branch_id", "owner_name"],
        "roles" => &["name", "branch_id", "protected"],
        "endpoints" | "endpoint" => &["id", "branch_id", "type", "current_state", "host"],
        _ => return Err(Error::InvalidInput),
    };
    let project = |row: &Value| -> Result<Value> {
        if !row.is_object()
            || !(safe_text(&row[if kind == "roles" { "name" } else { "id" }])
                || kind == "databases" && row["id"].is_u64())
        {
            return Err(Error::Unavailable);
        }
        let mut safe = serde_json::Map::new();
        for key in fields {
            if let Some(value) = row.get(*key) {
                if value.is_null()
                    || safe_text(value)
                    || (*key == "protected" && value.is_boolean())
                    || (*key == "id" && value.is_u64())
                {
                    safe.insert((*key).into(), value.clone());
                } else {
                    return Err(Error::Unavailable);
                }
            }
        }
        Ok(Value::Object(safe))
    };
    if ["project", "branch", "endpoint"].contains(&kind) {
        let safe = project(&raw[kind])?;
        let expected = path.rsplit('/').next().ok_or(Error::InvalidInput)?;
        if safe["id"] != expected {
            return Err(Error::InvalidScope);
        }
        return Ok(json!({kind:safe}));
    }
    if raw
        .get("unavailable_project_ids")
        .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
    {
        return Err(Error::Unavailable);
    }
    let rows = raw[kind]
        .as_array()
        .filter(|rows| rows.len() <= 100)
        .ok_or(Error::Unavailable)?;
    let rows = rows.iter().map(project).collect::<Result<Vec<_>>>()?;
    let cursor = raw["pagination"]["cursor"].clone();
    if !cursor.is_null()
        && !cursor
            .as_str()
            .is_some_and(|s| !s.is_empty() && s.len() <= 1024 && !s.chars().any(char::is_control))
    {
        return Err(Error::Unavailable);
    }
    Ok(json!({kind:rows,"pagination":{"cursor":cursor}}))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Attachment {
    name: String,
    neon_project_id: String,
    branch_id: String,
    database_name: String,
    role_name: String,
    endpoint_id: String,
}
impl Attachment {
    fn parse(input: Value) -> Result<Self> {
        let row: Self = serde_json::from_value(input).map_err(|_| Error::InvalidInput)?;
        if row.name.is_empty()
            || row.name.chars().count() > 160
            || row.name.chars().any(char::is_control)
            || ![&row.neon_project_id, &row.branch_id, &row.endpoint_id]
                .iter()
                .all(|v| neon::identifier(v))
            || ![&row.database_name, &row.role_name]
                .iter()
                .all(|v| !v.is_empty() && v.len() <= 63 && !v.chars().any(char::is_control))
        {
            return Err(Error::InvalidInput);
        }
        Ok(row)
    }
}
struct Snapshot {
    connection: Value,
    binding: Option<Value>,
    key: neon::ApiKey,
}
impl Connections {
    async fn database_snapshot(
        &self,
        context: &HttpAdapterContext<'_>,
        require_binding: bool,
    ) -> Result<Snapshot> {
        let guard = self
            .guard(
                context.principal(),
                context.authority(),
                &context.scope().customer_id,
                context.requires_connection_management(),
            )
            .await?;
        let connection = load(&guard.business, &context.scope().customer_id)
            .await?
            .filter(|row| row["status"] == "connected")
            .ok_or(Error::Conflict)?;
        let key = self
            .cipher
            .open(
                field(&connection, "encrypted_key")?,
                &purpose(
                    &context.scope().customer_id,
                    field(&connection, "id")?,
                    revision(Some(&connection)),
                    connection["neon_project_id"].as_str(),
                ),
            )
            .map_err(|_| Error::Unavailable)?;
        let binding = if require_binding {
            Some(
                self.database_binding(&guard, context)
                    .await?
                    .ok_or(Error::Conflict)?,
            )
        } else {
            None
        };
        self.finish(guard).await?;
        Ok(Snapshot {
            connection,
            binding,
            key: neon::ApiKey::new(key)?,
        })
    }
    async fn database_binding(
        &self,
        guard: &Guard,
        context: &HttpAdapterContext<'_>,
    ) -> Result<Option<Value>> {
        one(&guard.business,"SELECT to_jsonb(b) AS value FROM webdock_auth.neon_database_attachment b WHERE resource_id=$1 AND customer_id=$2 AND project_id=$3 AND detached_at IS NULL",vec![context.resource_id().ok_or(Error::InvalidInput)?.into(), context.scope().customer_id.clone().into(), context.scope().project_id.clone().into()]).await?
            .map(|row| row.try_get("", "value").map_err(Error::from)).transpose()
    }
    async fn recheck_database(
        &self,
        context: &HttpAdapterContext<'_>,
        snapshot: &Snapshot,
    ) -> Result<Guard> {
        context.revalidate().await.map_err(|_| Error::Forbidden)?;
        let guard = self
            .guard(
                context.principal(),
                context.authority(),
                &context.scope().customer_id,
                context.requires_connection_management(),
            )
            .await?;
        let connection = load(&guard.business, &context.scope().customer_id)
            .await?
            .ok_or(Error::Conflict)?;
        if connection["id"] != snapshot.connection["id"]
            || connection["revision"] != snapshot.connection["revision"]
            || connection["status"] != "connected"
        {
            return Err(Error::Conflict);
        }
        if let Some(binding) = &snapshot.binding {
            let current = self
                .database_binding(&guard, context)
                .await?
                .ok_or(Error::Conflict)?;
            if current != *binding {
                return Err(Error::Conflict);
            }
        }
        Ok(guard)
    }
    async fn inventory(
        &self,
        context: &HttpAdapterContext<'_>,
        snapshot: &Snapshot,
        path: &str,
        query: &Value,
    ) -> Result<Value> {
        let (selected, _) = inventory_path(path)?;
        // Verify exact project membership even for account keys, not merely possession of any key.
        if let Some(project) = selected {
            if snapshot.connection["neon_project_id"]
                .as_str()
                .is_some_and(|fixed| fixed != project)
            {
                return Err(Error::Forbidden);
            }
            let verified = self
                .provider
                .inventory(
                    &snapshot.key,
                    snapshot.connection["organization_id"].as_str(),
                    Some(project),
                    None,
                )
                .await?;
            if context.action() != "databases.discover" {
                require_database_region(verified.project_region(project))?;
            }
            let guard = self.recheck_database(context, snapshot).await?;
            self.finish(guard).await?;
        } else if snapshot.connection["neon_project_id"].is_string() {
            let project = field(&snapshot.connection, "neon_project_id")?;
            let raw = self
                .provider
                .get_json(&snapshot.key, &format!("/projects/{project}"), &[])
                .await?;
            if raw["project"]["id"] != project
                || snapshot.connection["organization_id"]
                    .as_str()
                    .is_some_and(|org| raw["project"]["org_id"] != org)
            {
                return Err(Error::InvalidScope);
            }
            let result =
                project_inventory("/projects", json!({"projects":[raw["project"].clone()]}))?;
            let guard = self.recheck_database(context, snapshot).await?;
            self.finish(guard).await?;
            return Ok(result);
        }
        let mut query_owned: Vec<(String, String)> = query
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| Ok((k.clone(), v.as_str().ok_or(Error::InvalidInput)?.to_owned())))
            .collect::<Result<_>>()?;
        if selected.is_none()
            && let Some(org) = snapshot.connection["organization_id"].as_str()
        {
            query_owned.push(("org_id".into(), org.into()));
        }
        let query: Vec<(&str, &str)> = query_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let result = project_inventory(
            path,
            self.provider.get_json(&snapshot.key, path, &query).await?,
        )?;
        let guard = self.recheck_database(context, snapshot).await?;
        self.finish(guard).await?;
        Ok(result)
    }
    async fn validate_tuple(
        &self,
        context: &HttpAdapterContext<'_>,
        snapshot: &Snapshot,
        tuple: &Attachment,
    ) -> Result<()> {
        let project = &tuple.neon_project_id;
        let branch = &tuple.branch_id;
        let current = self
            .inventory(
                context,
                snapshot,
                &format!("/projects/{project}/branches/{branch}"),
                &Value::Null,
            )
            .await?;
        if current["branch"]["id"] != *branch || current["branch"]["project_id"] != *project {
            return Err(Error::InvalidScope);
        }
        for (kind, name) in [
            ("databases", &tuple.database_name),
            ("roles", &tuple.role_name),
        ] {
            let rows = self
                .inventory(
                    context,
                    snapshot,
                    &format!("/projects/{project}/branches/{branch}/{kind}"),
                    &Value::Null,
                )
                .await?;
            if !rows[kind].as_array().is_some_and(|rows| {
                rows.iter()
                    .any(|r| r["name"] == *name && r["branch_id"] == *branch)
            }) {
                return Err(Error::InvalidScope);
            }
        }
        let endpoint = self
            .inventory(
                context,
                snapshot,
                &format!("/projects/{project}/endpoints/{}", tuple.endpoint_id),
                &Value::Null,
            )
            .await?;
        if endpoint["endpoint"]["id"] != tuple.endpoint_id
            || endpoint["endpoint"]["branch_id"] != *branch
        {
            return Err(Error::InvalidScope);
        }
        Ok(())
    }
    async fn database_request(
        &self,
        context: &HttpAdapterContext<'_>,
        input: Value,
    ) -> Result<Value> {
        let path = input["path"].as_str().ok_or(Error::InvalidInput)?;
        let method = input["method"].as_str().ok_or(Error::InvalidInput)?;
        let action = match (method, path) {
            ("POST", "/_spitzli/databases/attach") => "databases.attach",
            ("GET", "/_spitzli/databases/current") => "databases.view",
            ("GET", "/_spitzli/databases/branches") => "databases.branches",
            ("POST", "/_spitzli/databases/credentials") => "databases.credentials",
            ("POST", "/_spitzli/databases/detach") => "databases.detach",
            ("GET", _) if inventory_path(path).is_ok() => "databases.discover",
            _ => return Err(Error::Forbidden),
        };
        if context.scope().plugin_id != "neon"
            || context.action() != action
            || (method == "POST"
                && (!context.write()
                    || !context
                        .matches_input(&input["body"])
                        .map_err(|_| Error::Forbidden)?))
            || (path.starts_with("/_spitzli/") && input.get("query").is_some())
        {
            return Err(Error::Forbidden);
        }
        if action == "databases.detach" {
            return self.detach_database(context).await;
        }
        let snapshot = self
            .database_snapshot(
                context,
                !matches!(action, "databases.attach" | "databases.discover"),
            )
            .await?;
        match action {
            "databases.discover" => {
                self.inventory(context, &snapshot, path, &input["query"])
                    .await
            }
            "databases.attach" => {
                let tuple = Attachment::parse(input["body"].clone())?;
                self.validate_tuple(context, &snapshot, &tuple).await?;
                let guard = self.recheck_database(context, &snapshot).await?;
                let result = self
                    .commit_database(&guard, context, &snapshot.connection, None, &input["body"])
                    .await?;
                self.finish(guard).await?;
                Ok(result)
            }
            "databases.view" => {
                let row = snapshot.binding.as_ref().ok_or(Error::Conflict)?;
                let project = field(row, "neon_project_id")?;
                // The endpoint inventory call already validates this exact project's account and region.
                let endpoint = self
                    .inventory(
                        context,
                        &snapshot,
                        &format!(
                            "/projects/{project}/endpoints/{}",
                            field(row, "endpoint_id")?
                        ),
                        &Value::Null,
                    )
                    .await?;
                if endpoint["endpoint"]["branch_id"] != row["branch_id"] {
                    return Err(Error::InvalidScope);
                }
                let result = json!({"resource":{"id":row["resource_id"],"name":row["name"]},"binding":{"neon_project_id":row["neon_project_id"],"branch_id":row["branch_id"],"database_name":row["database_name"],"role_name":row["role_name"],"endpoint_id":row["endpoint_id"]},"status":field(&endpoint["endpoint"],"current_state")?});
                let guard = self.recheck_database(context, &snapshot).await?;
                self.finish(guard).await?;
                Ok(result)
            }
            "databases.branches" => {
                let project = field(
                    snapshot.binding.as_ref().ok_or(Error::Conflict)?,
                    "neon_project_id",
                )?;
                self.inventory(
                    context,
                    &snapshot,
                    &format!("/projects/{project}/branches"),
                    &json!({"limit":"100"}),
                )
                .await
            }
            "databases.credentials" => {
                self.database_credentials(context, &snapshot, &input["body"])
                    .await
            }
            _ => Err(Error::Forbidden),
        }
    }
    async fn database_credentials(
        &self,
        context: &HttpAdapterContext<'_>,
        snapshot: &Snapshot,
        body: &Value,
    ) -> Result<Value> {
        if body.as_object().is_none_or(|body| {
            body.keys()
                .any(|k| !["resource_id", "pooled"].contains(&k.as_str()))
        }) {
            return Err(Error::InvalidInput);
        }
        let pooled = match body.get("pooled") {
            None => true,
            Some(v) => v.as_bool().ok_or(Error::InvalidInput)?,
        };
        let row = snapshot.binding.as_ref().ok_or(Error::Conflict)?;
        let project = field(row, "neon_project_id")?;
        // Check project/org scope immediately before the only secret-bearing provider GET.
        self.inventory(
            context,
            snapshot,
            &format!("/projects/{project}"),
            &Value::Null,
        )
        .await?;
        let query = [
            ("branch_id", field(row, "branch_id")?),
            ("database_name", field(row, "database_name")?),
            ("role_name", field(row, "role_name")?),
            ("endpoint_id", field(row, "endpoint_id")?),
            ("pooled", if pooled { "true" } else { "false" }),
        ];
        let raw = self
            .provider
            .get_json(
                &snapshot.key,
                &format!("/projects/{project}/connection_uri"),
                &query,
            )
            .await?;
        let uri = raw["uri"]
            .as_str()
            .filter(|uri| {
                !uri.is_empty() && uri.len() <= 8192 && !uri.chars().any(char::is_control)
            })
            .ok_or(Error::Unavailable)?;
        validate_uri(uri, row, pooled)?;
        let guard = self.recheck_database(context, snapshot).await?;
        self.finish(guard).await?;
        Ok(json!({"value":uri}))
    }
    async fn commit_database(
        &self,
        guard: &Guard,
        context: &HttpAdapterContext<'_>,
        connection: &Value,
        binding: Option<&Value>,
        input: &Value,
    ) -> Result<Value> {
        one(
            &guard.business,
            "SELECT webdock_rls.commit_neon_database($1::uuid,$2::uuid,$3,$4,$5) AS value",
            vec![
                context.invocation_id().into(),
                field(connection, "id")?.into(),
                revision(Some(connection)).into(),
                binding.map(|b| b["revision"].as_i64().unwrap_or(-1)).into(),
                input.clone().into(),
            ],
        )
        .await?
        .ok_or(Error::Conflict)?
        .try_get("", "value")
        .map_err(Error::from)
    }
    async fn detach_database(&self, context: &HttpAdapterContext<'_>) -> Result<Value> {
        context.revalidate().await.map_err(|_| Error::Forbidden)?;
        let guard = self
            .guard(
                context.principal(),
                context.authority(),
                &context.scope().customer_id,
                true,
            )
            .await?;
        // Disconnecting an attachment is local and remains possible with a disconnected provider key.
        let connection = load(&guard.business, &context.scope().customer_id)
            .await?
            .ok_or(Error::Conflict)?;
        let binding = self
            .database_binding(&guard, context)
            .await?
            .ok_or(Error::Conflict)?;
        let result = self
            .commit_database(
                &guard,
                context,
                &connection,
                Some(&binding),
                &json!({"resource_id":context.resource_id()}),
            )
            .await?;
        self.finish(guard).await?;
        Ok(result)
    }
}
#[async_trait]
impl NamedHttpAdapter for Connections {
    async fn request(
        &self,
        context: &HttpAdapterContext<'_>,
        input: Value,
    ) -> std::result::Result<HttpResult, webdock_plugin_host::PluginError> {
        self.database_request(context, input)
            .await
            .map(|body| HttpResult { status: 200, body })
            .map_err(|error| {
                use webdock_plugin_host::{ErrorCode, PluginError};
                let code = match error {
                    Error::Forbidden => ErrorCode::PermissionDenied,
                    Error::Conflict => ErrorCode::Conflict,
                    Error::InvalidInput | Error::UnsupportedDatabaseRegion => {
                        ErrorCode::InvalidInput
                    }
                    Error::InvalidCredentials | Error::InvalidScope => ErrorCode::PermissionDenied,
                    Error::Unavailable => ErrorCode::Unavailable,
                };
                PluginError::new(code, error.to_string())
            })
    }
}

pub(crate) fn validate_uri(uri: &str, binding: &Value, pooled: bool) -> Result<()> {
    if uri.len() > 8000 || uri.trim() != uri || uri.chars().any(char::is_control) {
        return Err(Error::Unavailable);
    }
    let parsed = reqwest::Url::parse(uri).map_err(|_| Error::Unavailable)?;
    let mut expected =
        reqwest::Url::parse("postgresql://fixture.invalid").map_err(|_| Error::Unavailable)?;
    expected
        .set_username(field(binding, "role_name")?)
        .map_err(|_| Error::Unavailable)?;
    expected
        .path_segments_mut()
        .map_err(|_| Error::Unavailable)?
        .clear()
        .push(field(binding, "database_name")?);
    let endpoint = field(binding, "endpoint_id")?;
    let prefix = format!("{endpoint}{}.", if pooled { "-pooler" } else { "" });
    // libpq query parameters can override authority/path components. Never accept
    // provider-supplied destinations or identities outside the stored tuple.
    let mut ssl = false;
    let mut channel_binding = false;
    for (name, value) in parsed.query_pairs() {
        match name.as_ref() {
            "sslmode" if !ssl && value == "require" => ssl = true,
            "channel_binding" if !channel_binding && value == "require" => channel_binding = true,
            _ => return Err(Error::Unavailable),
        }
    }
    if !["postgres", "postgresql"].contains(&parsed.scheme())
        || parsed.password().is_none_or(str::is_empty)
        || parsed.username() != expected.username()
        || parsed.path() != expected.path()
        || parsed.fragment().is_some()
        || parsed.port().is_some_and(|port| port != 5432)
        || parsed
            .host_str()
            .is_none_or(|host| !endpoint_hostname(host, &prefix))
        || !ssl
    {
        return Err(Error::Unavailable);
    }
    Ok(())
}

// PostgreSQL URLs have opaque hosts in WHATWG URL parsers, while libpq decodes
// commas as a multi-host list. Accept only a single ordinary DNS hostname.
fn endpoint_hostname(host: &str, prefix: &str) -> bool {
    host.len() <= 253
        && host.starts_with(prefix)
        && host.ends_with(".neon.tech")
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

// Initial managed attachment policy: actual provider metadata must prove the
// Frankfurt region; inventory may still show other projects without attaching.
pub(crate) fn require_database_region(region: Option<&str>) -> Result<()> {
    if region != Some("aws-eu-central-1") {
        return Err(Error::UnsupportedDatabaseRegion);
    }
    Ok(())
}
