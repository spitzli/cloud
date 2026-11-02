use crate::{Result, error};
use serde::Deserialize;
#[derive(Clone, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContainerSettings {
    pub strategy: String,
    pub context_directory: String,
    pub dockerfile: String,
    #[serde(default)]
    pub build_args: std::collections::BTreeMap<String, String>,
}
impl Default for ContainerSettings {
    fn default() -> Self {
        Self {
            strategy: "auto".into(),
            context_directory: ".".into(),
            dockerfile: "Dockerfile".into(),
            build_args: Default::default(),
        }
    }
}
impl ContainerSettings {
    pub fn validate(&self) -> Result<()> {
        if !["auto", "dockerfile", "railpack"].contains(&self.strategy.as_str()) {
            return Err(error("invalid_build_strategy"));
        }
        if self.context_directory != "." {
            crate::artifact::safe_path(&self.context_directory)?;
        }
        crate::artifact::safe_path(&self.dockerfile)?;
        if self.build_args.keys().any(|key| {
            [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "NO_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
                "no_proxy",
            ]
            .contains(&key.as_str())
                || [
                    "BUILDKIT_",
                    "RAILPACK_",
                    "MISE_",
                    "XDG_",
                    "AZURE_",
                    "GOOGLE_",
                    "LD_",
                    "DYLD_",
                ]
                .iter()
                .any(|prefix| key.starts_with(prefix))
        }) {
            return Err(error("invalid_environment"));
        }
        crate::execution::GuestRequest {
            recipe: "dockerfile".into(),
            root_directory: ".".into(),
            build_environment: self.build_args.clone(),
            limits: crate::execution::Limits {
                cpu: 2,
                memory_mi_b: 4096,
                disk_bytes: 1024 * 1024 * 1024,
                duration_seconds: 1500,
            },
        }
        .validate()
    }
}
#[derive(Clone, Copy, Debug, serde::Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceWebhookTrigger {
    Push,
    WorkflowRun,
}
impl SourceWebhookTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Push => "push",
            Self::WorkflowRun => "workflow_run",
        }
    }
}

// One bounded command per request; keeping the wire enum direct avoids allocation indirection.
#[expect(
    clippy::large_enum_variant,
    reason = "One bounded wire command per request avoids allocation indirection."
)]
#[derive(Deserialize)]
#[serde(tag = "action", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum BuildCommand {
    #[serde(rename = "git.sources.list")]
    SourcesList {
        #[serde(rename = "projectID")]
        project_id: String,
    },
    #[serde(rename = "git.source.get")]
    SourceGet {
        #[serde(rename = "projectID")]
        project_id: String,
        #[serde(default, rename = "targetID")]
        target_id: Option<String>,
    },
    #[serde(rename = "git.source.configure")]
    SourceConfigure {
        #[serde(rename = "projectID")]
        project_id: String,
        #[serde(rename = "targetID")]
        target_id: String,
        #[serde(rename = "connectionID")]
        connection_id: String,
        #[serde(rename = "repositoryID")]
        repository_id: String,
        branch: String,
        root_directory: String,
        recipe: String,
        #[serde(default = "health")]
        health_path: String,
        #[serde(default = "provider")]
        build_provider: String,
        #[serde(default = "execution_backend")]
        execution_backend: String,
        #[serde(default)]
        container_build: Option<ContainerSettings>,
        #[serde(default)]
        workflow_path: Option<String>,
        #[serde(default)]
        webhook_trigger: Option<SourceWebhookTrigger>,
        #[serde(default = "prefix")]
        artifact_prefix: String,
        #[serde(default)]
        build_environment: Option<serde_json::Value>,
        revision: i32,
        #[serde(default = "yes")]
        enabled: bool,
        #[serde(default)]
        auto_publish: bool,
    },
    #[serde(rename = "git.source.webhook.configure")]
    SourceWebhook {
        #[serde(rename = "projectID")]
        project_id: String,
        #[serde(rename = "sourceID")]
        source_id: String,
        revision: i32,
        enabled: bool,
    },
    #[serde(rename = "git.builds.request")]
    BuildRequest {
        #[serde(rename = "projectID")]
        project_id: String,
        #[serde(default, rename = "targetID")]
        target_id: Option<String>,
        #[serde(default)]
        #[serde(rename = "sourceSHA")]
        source_sha: Option<String>,
        #[serde(default = "production")]
        environment_name: String,
        idempotency_key: String,
    },
    #[serde(rename = "git.builds.list")]
    BuildsList {
        #[serde(rename = "projectID")]
        project_id: String,
        #[serde(default)]
        #[serde(rename = "sourceID")]
        source_id: Option<String>,
        #[serde(default = "page")]
        page: u32,
        #[serde(default = "limit")]
        limit: u32,
    },
    #[serde(rename = "git.builds.get")]
    BuildGet {
        #[serde(rename = "projectID")]
        project_id: String,
        #[serde(rename = "buildID")]
        build_id: String,
    },
    #[serde(rename = "git.builds.logs")]
    BuildLogs {
        #[serde(rename = "projectID")]
        project_id: String,
        #[serde(rename = "buildID")]
        build_id: String,
        #[serde(default = "cursor")]
        after: String,
        #[serde(default = "limit")]
        limit: u32,
    },
    #[serde(rename = "git.builds.cancel")]
    BuildCancel {
        #[serde(rename = "projectID")]
        project_id: String,
        #[serde(rename = "buildID")]
        build_id: String,
    },
    #[serde(rename = "git.releases.list")]
    ReleasesList {
        #[serde(rename = "projectID")]
        project_id: String,
        #[serde(default)]
        #[serde(rename = "sourceID")]
        source_id: Option<String>,
        #[serde(default = "page")]
        page: u32,
        #[serde(default = "limit")]
        limit: u32,
    },
    #[serde(rename = "git.releases.approve")]
    ReleaseApprove {
        #[serde(rename = "projectID")]
        project_id: String,
        #[serde(rename = "releaseID")]
        release_id: String,
        revision: i32,
        idempotency_key: String,
    },
    #[serde(rename = "git.releases.rollback")]
    ReleaseRollback {
        #[serde(rename = "projectID")]
        project_id: String,
        #[serde(rename = "releaseID")]
        release_id: String,
        revision: i32,
        idempotency_key: String,
    },
}
fn health() -> String {
    "/".into()
}
fn provider() -> String {
    "isolated".into()
}
pub(crate) fn workflow() -> String {
    ".github/workflows/webdock.yml".into()
}
fn prefix() -> String {
    "webdock".into()
}
fn yes() -> bool {
    true
}
fn production() -> String {
    "production".into()
}
fn cursor() -> String {
    "0".into()
}
fn execution_backend() -> String {
    "microvm".into()
}
fn page() -> u32 {
    1
}
fn limit() -> u32 {
    25
}
pub fn id(s: &str) -> Result<()> {
    if !s.bytes().all(|b| b.is_ascii_digit())
        || s.starts_with('0')
        || s.parse::<i64>().is_err()
        || s.parse::<i64>().unwrap_or(0) <= 0
    {
        return Err(error("invalid_id"));
    }
    Ok(())
}
pub fn sha(s: &str) -> Result<()> {
    if s.len() != 40
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(error("invalid_commit"));
    }
    Ok(())
}
pub fn key(s: &str) -> Result<()> {
    if !(16..=128).contains(&s.len())
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(error("invalid_idempotency_key"));
    }
    Ok(())
}
impl BuildCommand {
    pub fn project(&self) -> &str {
        match self {
            Self::SourceGet { project_id, .. }
            | Self::SourcesList { project_id }
            | Self::SourceConfigure { project_id, .. }
            | Self::SourceWebhook { project_id, .. }
            | Self::BuildRequest { project_id, .. }
            | Self::BuildsList { project_id, .. }
            | Self::BuildGet { project_id, .. }
            | Self::BuildLogs { project_id, .. }
            | Self::BuildCancel { project_id, .. }
            | Self::ReleasesList { project_id, .. }
            | Self::ReleaseApprove { project_id, .. }
            | Self::ReleaseRollback { project_id, .. } => project_id,
        }
    }
    pub fn write(&self) -> bool {
        matches!(
            self,
            Self::SourceConfigure { .. }
                | Self::SourceWebhook { .. }
                | Self::BuildRequest { .. }
                | Self::BuildCancel { .. }
                | Self::ReleaseApprove { .. }
                | Self::ReleaseRollback { .. }
        )
    }
    pub fn parse(value: serde_json::Value) -> Result<Self> {
        if serde_json::to_vec(&value)
            .map_err(|_| error("invalid_command"))?
            .len()
            > 65536
        {
            return Err(error("command_limit"));
        }
        for optional in [
            "targetID",
            "sourceID",
            "sourceSHA",
            "buildEnvironment",
            "executionBackend",
            "containerBuild",
            "environmentName",
        ] {
            if value.get(optional).is_some_and(serde_json::Value::is_null) {
                return Err(error("invalid_command"));
            }
        }
        let c: Self = serde_json::from_value(value).map_err(|_| error("invalid_command"))?;
        id(c.project())?;
        match &c {
            Self::SourceConfigure {
                target_id,
                connection_id,
                repository_id,
                branch,
                root_directory,
                recipe,
                revision,
                health_path,
                build_provider,
                execution_backend,
                container_build,
                workflow_path,
                webhook_trigger,
                artifact_prefix,
                build_environment,
                ..
            } => {
                id(target_id)?;
                id(connection_id)?;
                id(repository_id)?;
                if !["microvm", "kubernetes"].contains(&execution_backend.as_str()) {
                    return Err(error("invalid_execution_backend"));
                }
                if let Some(settings) = container_build {
                    if execution_backend != "kubernetes" || recipe != "dockerfile" {
                        return Err(error("invalid_build_strategy"));
                    }
                    settings.validate()?;
                }
                if execution_backend == "kubernetes" && recipe != "dockerfile" {
                    return Err(error("recipe_unavailable"));
                }
                if build_provider != "isolated" {
                    return Err(error("build_provider_unavailable"));
                }
                if !health_path.starts_with('/')
                    || health_path.contains("//")
                    || health_path.len() > 160
                    || !health_path
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"/_-".contains(&b))
                {
                    return Err(error("invalid_health_path"));
                }
                if *webhook_trigger == Some(SourceWebhookTrigger::WorkflowRun)
                    && execution_backend != "kubernetes"
                {
                    return Err(error("invalid_webhook_trigger"));
                }
                let workflow_path = workflow_path.clone().unwrap_or_else(workflow);
                let workflow = workflow_path
                    .strip_prefix(".github/workflows/")
                    .and_then(|p| p.strip_suffix(".yml").or_else(|| p.strip_suffix(".yaml")));
                if workflow.is_none_or(|p| {
                    p.is_empty()
                        || !p
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                }) || artifact_prefix.is_empty()
                    || artifact_prefix.len() > 80
                    || !artifact_prefix
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                {
                    return Err(error("invalid_source_configuration"));
                }
                if let Some(patch) = build_environment
                    && (!patch.is_array() || patch.as_array().unwrap().len() > 32)
                {
                    return Err(error("invalid_environment"));
                }
                if repository_id.is_empty()
                    || repository_id.len() > 200
                    || repository_id.chars().any(char::is_control)
                    || *revision < 0
                    || *revision == i32::MAX
                    || !["dockerfile", crate::frontend::RECIPE].contains(&recipe.as_str())
                {
                    return Err(error("source_unavailable"));
                }
                if branch.is_empty()
                    || branch.len() > 200
                    || branch
                        .as_bytes()
                        .first()
                        .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_')
                    || !branch
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
                    || branch.contains("..")
                    || branch.contains("//")
                    || branch.ends_with('/')
                    || branch.ends_with(".lock")
                {
                    return Err(error("invalid_branch"));
                }
                if root_directory.len() > 240
                    || !root_directory
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_./-".contains(&b))
                {
                    return Err(error("invalid_root"));
                }
                if root_directory != "." {
                    crate::artifact::safe_path(root_directory)?;
                }
            }
            Self::SourceWebhook {
                source_id,
                revision,
                ..
            } => {
                id(source_id)?;
                if *revision <= 0 || *revision == i32::MAX {
                    return Err(error("invalid_revision"));
                }
            }
            Self::BuildRequest {
                target_id,
                source_sha,
                environment_name,
                idempotency_key,
                ..
            } => {
                if environment_name != "production" {
                    return Err(error("invalid_build_environment"));
                }
                if let Some(target) = target_id {
                    id(target)?;
                }
                key(idempotency_key)?;
                if let Some(s) = source_sha {
                    sha(s)?;
                }
            }
            Self::BuildsList {
                source_id,
                page,
                limit,
                ..
            }
            | Self::ReleasesList {
                source_id,
                page,
                limit,
                ..
            } => {
                if let Some(s) = source_id {
                    id(s)?;
                }
                if !(1..=100000).contains(page) || !(1..=100).contains(limit) {
                    return Err(error("invalid_pagination"));
                }
            }
            Self::BuildLogs {
                build_id,
                after,
                limit,
                ..
            } => {
                id(build_id)?;
                if after != "0" {
                    id(after)?;
                }
                if !(1..=100).contains(limit) {
                    return Err(error("invalid_pagination"));
                }
            }
            Self::BuildGet { build_id, .. } | Self::BuildCancel { build_id, .. } => id(build_id)?,
            Self::ReleaseRollback {
                release_id,
                revision,
                idempotency_key,
                ..
            }
            | Self::ReleaseApprove {
                release_id,
                revision,
                idempotency_key,
                ..
            } => {
                id(release_id)?;
                key(idempotency_key)?;
                if *revision < 0 || *revision == i32::MAX {
                    return Err(error("invalid_revision"));
                }
            }
            Self::SourceGet {
                target_id: Some(target),
                ..
            } => {
                id(target)?;
            }
            _ => {}
        }
        Ok(c)
    }
}

/// Explicit read allowlist shared by browser ceilings and deployment feature gates.
pub fn read_action(action: &str) -> bool {
    matches!(
        action,
        "git.connections.list"
            | "git.repositories.list"
            | "git.sources.list"
            | "git.source.get"
            | "git.builds.list"
            | "git.builds.get"
            | "git.builds.logs"
            | "git.releases.list"
    )
}
#[derive(Clone, Copy)]
pub struct BuildCeiling {
    pub read: bool,
    pub write: bool,
}
impl BuildCeiling {
    pub fn require(self, action: &str) -> Result<()> {
        if !self.read || (!read_action(action) && !self.write) {
            return Err(error("permission_denied"));
        }
        Ok(())
    }
}
