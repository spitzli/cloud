use super::lifecycle::recovery_env;
use super::*;
use kube::api::DynamicObject;
use serde_json::{Value, json};
pub(super) fn validate_workload(
    object: &DynamicObject,
    claim: &MailClaim,
    config: &KubernetesMailConfig,
) -> Result<()> {
    let spec = &object.data["spec"];
    let pod = &spec["template"]["spec"];
    let containers = pod["containers"]
        .as_array()
        .ok_or(ExecutionError::Ownership)?;
    if containers.len() != 1
        || containers[0]["name"] != "app"
        || containers[0]["image"] != config.image
        || containers[0]["args"] != json!(["--config", "/data/config.json"])
        || spec["selector"]["matchLabels"] != json!({"webdock.dev/app-id":claim.spec.customer_id})
        || spec["strategy"]["type"] != "Recreate"
        || pod["automountServiceAccountToken"] != false
        || ["hostNetwork", "hostPID", "hostIPC"]
            .iter()
            .any(|key| pod[*key] == true)
        || ["initContainers", "ephemeralContainers"]
            .iter()
            .any(|key| pod[*key].as_array().is_some_and(|v| !v.is_empty()))
    {
        return Err(ExecutionError::Ownership);
    }
    let legacy_never = containers[0]["imagePullPolicy"] == "Never"
        && object
            .metadata
            .labels
            .as_ref()
            .is_none_or(|labels| !labels.contains_key("webdock.dev/mail-installation"))
        && config
            .retained
            .get(&claim.spec.customer_id)
            .is_some_and(|binding| object.metadata.uid.as_ref() == Some(&binding.deployment_uid));
    if containers[0]["imagePullPolicy"] != "IfNotPresent" && !legacy_never {
        return Err(ExecutionError::Ownership);
    }
    let security = &containers[0]["securityContext"];
    if security["allowPrivilegeEscalation"] != false
        || security["readOnlyRootFilesystem"] != true
        || security["privileged"] == true
        || security["capabilities"]["drop"] != json!(["ALL"])
        || security["capabilities"]["add"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
        || pod["securityContext"]["runAsNonRoot"] != true
        || pod["securityContext"]["runAsUser"] != 65532
        || pod["securityContext"]["runAsGroup"] != 65532
        || pod["securityContext"]["fsGroup"] != 65532
        || pod["securityContext"]["seccompProfile"]["type"] != "RuntimeDefault"
    {
        return Err(ExecutionError::Ownership);
    }
    let volumes = pod["volumes"].as_array().ok_or(ExecutionError::Ownership)?;
    if volumes.len() != 2
        || volumes.iter().any(|v| v.get("hostPath").is_some())
        || !volumes.iter().any(|v| {
            v["name"] == "data"
                && v["persistentVolumeClaim"]["claimName"]
                    == format!("app-{}-data", claim.spec.customer_id)
        })
        || !volumes
            .iter()
            .any(|v| v["name"] == "tmp" && v.get("emptyDir").is_some())
    {
        return Err(ExecutionError::Ownership);
    }
    let env = containers[0]["env"]
        .as_array()
        .ok_or(ExecutionError::Ownership)?;
    let recovery = recovery_env(pod);
    if env.len() != if recovery { 3 } else { 1 }
        || !env.iter().any(|v| {
            v["name"] == "STALWART_PUBLIC_URL"
                && v["value"] == format!("https://{}", claim.spec.hostname)
        })
        || object.data["spec"]["template"]["metadata"]["labels"]
            .get("security.webdock.dev/ingress")
            .is_some()
    {
        return Err(ExecutionError::Ownership);
    }
    if recovery
        && (!env
            .iter()
            .any(|v| v["name"] == "STALWART_RECOVERY_MODE" && v["value"] == "1")
            || !env.iter().any(|v| {
                v["name"] == "STALWART_RECOVERY_ADMIN"
                    && v["valueFrom"]["secretKeyRef"]["key"] == "administrator"
                    && v["valueFrom"]["secretKeyRef"]["name"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("mail-bootstrap-"))
            }))
    {
        return Err(ExecutionError::Ownership);
    }
    effective_profile(object, claim, config)
}
fn only(value: &Value, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.keys().all(|key| keys.contains(&key.as_str())))
}
fn optional(value: &Value, key: &str, expected: &Value) -> bool {
    value
        .get(key)
        .is_none_or(|actual| actual.is_null() || actual == expected)
}
fn quantity(value: &Value, expected: u64, cpu: bool) -> bool {
    value.as_str().is_some_and(|value| {
        crate::kubernetes::parse_quantity(value, cpu).is_ok_and(|value| value == expected)
    })
}
fn effective_profile(
    object: &DynamicObject,
    claim: &MailClaim,
    config: &KubernetesMailConfig,
) -> Result<()> {
    let pod = &object.data["spec"]["template"]["spec"];
    let container = &pod["containers"][0];
    let retained = config
        .retained
        .get(&claim.spec.customer_id)
        .is_some_and(|binding| object.metadata.uid.as_ref() == Some(&binding.deployment_uid))
        && object
            .metadata
            .labels
            .as_ref()
            .is_none_or(|labels| !labels.contains_key("webdock.dev/mail-installation"));
    if !only(
        container,
        &[
            "name",
            "image",
            "imagePullPolicy",
            "args",
            "ports",
            "env",
            "securityContext",
            "resources",
            "readinessProbe",
            "livenessProbe",
            "startupProbe",
            "volumeMounts",
            "terminationMessagePath",
            "terminationMessagePolicy",
        ],
    ) || !optional(
        container,
        "terminationMessagePath",
        &json!("/dev/termination-log"),
    ) || !optional(container, "terminationMessagePolicy", &json!("File"))
        || !only(
            pod,
            &[
                "containers",
                "volumes",
                "automountServiceAccountToken",
                "serviceAccountName",
                "serviceAccount",
                "terminationGracePeriodSeconds",
                "securityContext",
                "nodeSelector",
                "nodeName",
                "dnsPolicy",
                "restartPolicy",
                "schedulerName",
                "enableServiceLinks",
                "preemptionPolicy",
                "priority",
                "tolerations",
                "hostNetwork",
                "hostPID",
                "hostIPC",
            ],
        )
        || pod["serviceAccountName"] != "default"
        || !optional(pod, "serviceAccount", &json!("default"))
        || pod["terminationGracePeriodSeconds"] != 15
        || !optional(pod, "dnsPolicy", &json!("ClusterFirst"))
        || !optional(pod, "restartPolicy", &json!("Always"))
        || !optional(pod, "schedulerName", &json!("default-scheduler"))
        || !optional(pod, "preemptionPolicy", &json!("PreemptLowerPriority"))
        || !optional(pod, "priority", &json!(0))
        || (!retained || pod.get("nodeSelector").is_some())
            && pod["nodeSelector"] != json!({"kubernetes.io/hostname":config.node})
        || pod
            .get("nodeName")
            .is_some_and(|node| !node.is_null() && node != "" && node != &json!(config.node))
    {
        return Err(ExecutionError::Ownership);
    }
    let security = &container["securityContext"];
    if !only(
        security,
        &[
            "allowPrivilegeEscalation",
            "readOnlyRootFilesystem",
            "capabilities",
            "privileged",
            "runAsNonRoot",
            "runAsUser",
            "runAsGroup",
            "seccompProfile",
            "appArmorProfile",
        ],
    ) || !optional(security, "runAsNonRoot", &json!(true))
        || !optional(security, "runAsUser", &json!(65532))
        || !optional(security, "runAsGroup", &json!(65532))
        || !optional(
            security,
            "seccompProfile",
            &json!({"type":"RuntimeDefault"}),
        )
        || !optional(
            security,
            "appArmorProfile",
            &json!({"type":"RuntimeDefault"}),
        )
        || !only(
            &pod["securityContext"],
            &[
                "runAsNonRoot",
                "runAsUser",
                "runAsGroup",
                "fsGroup",
                "seccompProfile",
                "appArmorProfile",
            ],
        )
        || !optional(
            &pod["securityContext"],
            "appArmorProfile",
            &json!({"type":"RuntimeDefault"}),
        )
        || pod["securityContext"]["seccompProfile"] != json!({"type":"RuntimeDefault"})
    {
        return Err(ExecutionError::Ownership);
    }
    if pod.get("tolerations").is_some_and(|values| {
        values.as_array().is_none_or(|tolerations| {
            tolerations.len() > 2
                || tolerations.iter().any(|value| {
                    !only(value, &["key", "operator", "effect", "tolerationSeconds"])
                        || ![
                            "node.kubernetes.io/not-ready",
                            "node.kubernetes.io/unreachable",
                        ]
                        .iter()
                        .any(|key| value["key"] == *key)
                        || value["operator"] != "Exists"
                        || value["effect"] != "NoExecute"
                        || value["tolerationSeconds"] != 300
                })
        })
    }) {
        return Err(ExecutionError::Ownership);
    }
    if !only(&container["resources"], &["requests", "limits"]) {
        return Err(ExecutionError::Ownership);
    }
    for category in ["requests", "limits"] {
        let resources = &container["resources"][category];
        if !only(resources, &["cpu", "memory", "ephemeral-storage"])
            || !quantity(&resources["cpu"], claim.spec.resources.cpu_millicores, true)
            || !quantity(
                &resources["memory"],
                claim.spec.resources.memory_bytes,
                false,
            )
            || !quantity(
                &resources["ephemeral-storage"],
                claim.spec.resources.ephemeral_bytes,
                false,
            )
        {
            return Err(ExecutionError::Ownership);
        }
    }
    mounts_and_ports(container)?;
    for volume in pod["volumes"].as_array().ok_or(ExecutionError::Ownership)? {
        if volume["name"] == "tmp" {
            if !only(volume, &["name", "emptyDir"])
                || !only(&volume["emptyDir"], &["sizeLimit", "medium"])
                || !optional(&volume["emptyDir"], "medium", &json!(""))
                || !quantity(
                    &volume["emptyDir"]["sizeLimit"],
                    claim.spec.resources.ephemeral_bytes,
                    false,
                )
            {
                return Err(ExecutionError::Ownership);
            }
        } else if !only(volume, &["name", "persistentVolumeClaim"])
            || !only(&volume["persistentVolumeClaim"], &["claimName", "readOnly"])
            || !optional(&volume["persistentVolumeClaim"], "readOnly", &json!(false))
        {
            return Err(ExecutionError::Ownership);
        }
    }
    for (name, failures) in [
        ("readinessProbe", 3),
        ("livenessProbe", 3),
        ("startupProbe", 20),
    ] {
        let probe = &container[name];
        if !only(
            probe,
            &[
                "httpGet",
                "periodSeconds",
                "timeoutSeconds",
                "failureThreshold",
                "initialDelaySeconds",
                "successThreshold",
            ],
        ) || probe["periodSeconds"] != 3
            || probe["timeoutSeconds"] != 2
            || probe["failureThreshold"] != failures
            || !optional(probe, "initialDelaySeconds", &json!(0))
            || !optional(probe, "successThreshold", &json!(1))
            || !only(&probe["httpGet"], &["path", "port", "scheme", "host"])
            || probe["httpGet"]["path"] != "/healthz/live"
            || probe["httpGet"]["port"] != 8080
            || !optional(&probe["httpGet"], "scheme", &json!("HTTP"))
            || !optional(&probe["httpGet"], "host", &json!(""))
        {
            return Err(ExecutionError::Ownership);
        }
    }
    Ok(())
}
fn mounts_and_ports(container: &Value) -> Result<()> {
    let mounts = container["volumeMounts"]
        .as_array()
        .ok_or(ExecutionError::Ownership)?;
    if mounts.len() != 2
        || mounts.iter().any(|value| {
            !only(value, &["name", "mountPath", "readOnly"])
                || !optional(value, "readOnly", &json!(false))
        })
        || !mounts
            .iter()
            .any(|value| value["name"] == "tmp" && value["mountPath"] == "/tmp")
        || !mounts
            .iter()
            .any(|value| value["name"] == "data" && value["mountPath"] == "/data")
    {
        return Err(ExecutionError::Ownership);
    }
    let ports = container["ports"]
        .as_array()
        .ok_or(ExecutionError::Ownership)?;
    if ports.len() != 4
        || ports.iter().any(|value| {
            !only(value, &["name", "containerPort", "protocol"])
                || !optional(value, "protocol", &json!("TCP"))
        })
        || [
            ("http", 8080),
            ("smtp", 2525),
            ("submissions", 2465),
            ("imaps", 1993),
        ]
        .iter()
        .any(|(name, port)| {
            !ports
                .iter()
                .any(|value| value["name"] == *name && value["containerPort"] == *port)
        })
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}
