//! Core alone determines which compiled services are actually configured and loaded.
use super::*;
use webdock_core::features::{FeatureSet, FeatureState};
use webdock_plugin_host::BuiltinModule;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActivateCommand {
    idempotency_key: String,
}

fn available(
    features: &FeatureSet,
    id: &str,
    hosting_loaded: bool,
    databases_loaded: bool,
) -> Result<BuiltinModule, PluginError> {
    match id {
        "hosting" if hosting_loaded && features.state(id) != FeatureState::Disabled => {
            Ok(BuiltinModule::Hosting)
        }
        "databases" if databases_loaded && features.state(id) != FeatureState::Disabled => {
            Ok(BuiltinModule::Databases)
        }
        _ => Err(PluginError::new(
            ErrorCode::CapabilityUnavailable,
            "Compiled builtin service is unavailable",
        )),
    }
}

pub(super) fn routes() -> Router<Arc<ServiceState>> {
    Router::new().route(
        "/api/console/plugins/builtins/{id}/activate",
        post(activate),
    )
}

async fn activate(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        writer(&actor)?;
        let input: ActivateCommand = command(&body, &headers)?;
        let module = available(
            &state.config.features,
            &id,
            state.hosting.is_some(),
            state.databases.is_some(),
        )
        .map_err(failure)?;
        let value = endpoints(&state)?
            .host
            .activate_builtin_with_authority(
                &actor.principal,
                &actor.authority,
                module,
                &input.idempotency_key,
            )
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use webdock_core::features::FeatureSet;

    #[test]
    fn only_loaded_non_disabled_hosting_and_databases_can_activate() {
        let features = FeatureSet::from_json(
            r#"{"hosting":"enabled","databases":"draining","mail":"enabled"}"#,
        )
        .unwrap();
        assert_eq!(
            available(&features, "hosting", true, false).unwrap(),
            BuiltinModule::Hosting
        );
        assert_eq!(
            available(&features, "databases", false, true).unwrap(),
            BuiltinModule::Databases
        );
        for id in ["mail", "archives", "object-storage", "HOSTING", "arbitrary"] {
            assert!(available(&features, id, true, true).is_err());
        }
        assert!(available(&features, "hosting", false, true).is_err());
        assert!(available(&features, "databases", true, false).is_err());
        let disabled = FeatureSet::default();
        assert!(available(&disabled, "hosting", true, true).is_err());
        assert!(available(&disabled, "databases", true, true).is_err());
    }

    #[test]
    fn client_cannot_supply_capability_identity_or_version() {
        assert!(serde_json::from_value::<ActivateCommand>(json!({"idempotencyKey":"one"})).is_ok());
        for field in [
            "available",
            "version",
            "kind",
            "pluginID",
            "image",
            "customerID",
            "projectID",
        ] {
            let mut value = json!({"idempotencyKey":"one"});
            value[field] = json!("untrusted");
            assert!(serde_json::from_value::<ActivateCommand>(value).is_err());
        }
    }
}
