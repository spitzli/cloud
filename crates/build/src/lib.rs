//! Isolated build execution and retained Git deployment persistence.
pub mod artifact;
pub mod contracts;
pub mod evidence;
pub mod frontend;
pub use contracts::BuildCeiling;
#[cfg(feature = "service")]
mod service;
#[cfg(feature = "service")]
pub use service::kubernetes::KubernetesStoragePolicy;
#[cfg(feature = "service")]
pub use service::repository::{
    FileRepositoryBindings, PluginRepositoryProvider, RepositoryAdmission, RepositoryAuthorityRpc,
    RepositoryBinding, RepositoryBindings, RepositoryTransport,
};
#[cfg(feature = "service")]
pub use service::{
    BuildService, RepositoryProvider, RepositoryRef, VerifiedCommit, WorkerIdentity,
};
#[cfg(all(feature = "kubernetes-job", feature = "worker"))]
pub mod build_storage;
#[cfg(feature = "kubernetes-job")]
pub mod container_runner;
pub mod execution;
#[cfg(feature = "service")]
pub mod http;
#[cfg(feature = "kubernetes-job")]
pub mod kubernetes;
#[cfg(all(feature = "kubernetes-job", feature = "worker"))]
pub mod kubernetes_dispatcher;
#[cfg(all(feature = "kubernetes-job", feature = "worker"))]
pub mod kubernetes_identity;
#[cfg(all(feature = "kubernetes-job", feature = "worker"))]
pub mod kubernetes_transfer;
#[cfg(all(feature = "kubernetes-job", feature = "worker"))]
pub mod kubernetes_worker;
#[cfg(any(feature = "service", feature = "worker"))]
pub mod publisher;
#[cfg(any(feature = "service", feature = "worker"))]
pub mod remote;
#[cfg(any(feature = "service", feature = "worker"))]
pub mod worker;
#[derive(Debug, thiserror::Error)]
#[error("{code}")]
pub struct Error {
    pub code: &'static str,
    pub logs: String,
}
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn error(code: &'static str) -> Error {
    Error {
        code,
        logs: String::new(),
    }
}
pub(crate) fn io(_: std::io::Error) -> Error {
    error("storage_unavailable")
}
impl Error {
    pub fn status(&self) -> u16 {
        match self.code {
            code if code.starts_with("invalid_") || code.ends_with("_limit") => 400,
            "authentication_required" | "worker_authentication_failed" => 401,
            "permission_denied"
            | "operator_required"
            | "worker_revoked"
            | "release_authority_revoked" => 403,
            "endpoint_unavailable"
            | "customer_unavailable"
            | "project_unavailable"
            | "source_unavailable"
            | "target_unavailable"
            | "build_unavailable"
            | "release_unavailable"
            | "repository_unavailable" => 404,
            "select_source_target" | "build_not_cancellable" | "build_publication_cancelled" => 409,
            code if code.ends_with("_conflict")
                || code.ends_with("_changed")
                || code.ends_with("_lost") =>
            {
                409
            }
            _ => 503,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::error;

    #[test]
    fn error_status_preserves_codes_and_pattern_precedence() {
        for (code, expected) in [
            ("invalid_input", 400),
            ("invalid_source_conflict", 400),
            ("invalid_target_changed", 400),
            ("invalid_lease_lost", 400),
            ("command_limit", 400),
            ("resource_limit", 400),
            ("authentication_required", 401),
            ("worker_authentication_failed", 401),
            ("permission_denied", 403),
            ("operator_required", 403),
            ("worker_revoked", 403),
            ("release_authority_revoked", 403),
            ("endpoint_unavailable", 404),
            ("customer_unavailable", 404),
            ("project_unavailable", 404),
            ("source_unavailable", 404),
            ("target_unavailable", 404),
            ("build_unavailable", 404),
            ("release_unavailable", 404),
            ("repository_unavailable", 404),
            ("source_conflict", 409),
            ("source_changed", 409),
            ("lease_lost", 409),
            ("select_source_target", 409),
            ("build_not_cancellable", 409),
            ("storage_unavailable", 503),
            ("invalid", 503),
            ("limit", 503),
            ("conflict", 503),
            ("invalidity_conflict", 409),
            ("source_conflicting", 503),
            ("worker_revoked_extra", 503),
            ("", 503),
        ] {
            assert_eq!(error(code).status(), expected, "{code}");
        }
    }
}

#[cfg(feature = "service")]
pub use service::github_webhook::{
    GithubWebhookVerifier, MAX_GITHUB_WEBHOOK_BODY, VerifiedGithubEvent,
};

#[cfg(all(feature = "kubernetes-job", feature = "worker"))]
pub mod kubernetes_staging;
#[cfg(all(feature = "kubernetes-job", feature = "worker"))]
pub mod kubernetes_upload;
