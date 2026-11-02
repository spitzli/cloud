use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use webdock_agent_protocol::mail::{MailAction, MailMaterial};
pub(super) fn hash(value: &impl Serialize) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(value).map_err(|_| ExecutionError::Invalid("Mail profile"))?
        )
    ))
}
pub(super) fn profile(claim: &MailClaim) -> Result<String> {
    let credentials = match &claim.material {
        MailMaterial::Credentials {
            username, password, ..
        } => hash(&(username, password))?,
        MailMaterial::Unallocated {} => return Err(ExecutionError::Denied),
    };
    hash(&(
        &claim.spec.customer_id,
        &claim.spec.cluster_id,
        &claim.spec.hostname,
        &claim.spec.image,
        claim.spec.resources,
        credentials,
    ))
}

pub(super) fn labels(
    claim: &MailClaim,
    config: &KubernetesMailConfig,
    app: bool,
) -> Result<BTreeMap<String, String>> {
    let mut labels = BTreeMap::from([
        ("security.webdock.dev/managed-by".into(), "webdock".into()),
        (
            "webdock.dev/project-id".into(),
            claim.spec.customer_id.clone(),
        ),
        (
            "webdock.dev/cluster-id".into(),
            claim.spec.cluster_id.clone(),
        ),
        (
            "webdock.dev/mail-installation".into(),
            config.installation.clone(),
        ),
    ]);
    if app {
        labels.insert("webdock.dev/app-id".into(), claim.spec.customer_id.clone());
    }
    Ok(labels)
}
fn decode<T: DeserializeOwned>(value: serde_json::Value) -> Result<T> {
    serde_json::from_value(value).map_err(|_| ExecutionError::Invalid("Mail manifest"))
}
pub fn mail_objects(
    claim: &MailClaim,
    config: &KubernetesMailConfig,
    storage: &crate::storage::StorageConfig,
    phase: MailPhase,
) -> Result<MailObjects> {
    claim
        .validate(chrono::Utc::now())
        .map_err(|_| ExecutionError::Denied)?;
    config.validate()?;
    storage.validate()?;
    if config.image != claim.spec.image
        || storage.node != config.node
        || storage.cluster_id.as_str() != claim.spec.cluster_id
    {
        return Err(ExecutionError::Ownership);
    }
    let namespace = format!("wd-mail-{}", claim.spec.customer_id);
    let name = format!("app-{}", claim.spec.customer_id);
    let pvc = format!("{name}-data");
    let pv = format!("wd-{}-data", claim.spec.customer_id);
    let owner = labels(claim, config, false)?;
    let app = labels(claim, config, true)?;
    let annotations = json!({"webdock.dev/revision":claim.spec.revision.to_string(),"webdock.dev/operation":claim.spec.operation_id,"webdock.dev/mail-generation":claim.spec.agent_generation.to_string(),"webdock.dev/mail-profile":profile(claim)?});
    let metadata =
        json!({"name":name,"namespace":namespace,"labels":app,"annotations":annotations});
    let resource = claim.spec.resources;
    let limits = json!({"cpu":format!("{}m",resource.cpu_millicores),"memory":resource.memory_bytes.to_string(),"ephemeral-storage":resource.ephemeral_bytes.to_string()});
    let ports = json!([{"name":"http","containerPort":8080},{"name":"smtp","containerPort":2525},{"name":"submissions","containerPort":2465},{"name":"imaps","containerPort":1993}]);
    let mut pod_labels = app.clone();
    pod_labels.remove("webdock.dev/mail-installation");
    pod_labels.insert(
        "webdock.dev/mail-customer-id".into(),
        claim.spec.customer_id.clone(),
    );
    let mut env = vec![
        json!({"name":"STALWART_PUBLIC_URL","value":format!("https://{}",claim.spec.hostname)}),
    ];
    let secret = if phase != MailPhase::Normal {
        let MailMaterial::Credentials {
            bootstrap_password: Some(password),
            ..
        } = &claim.material
        else {
            return Err(ExecutionError::Denied);
        };
        let secret_name = format!("mail-bootstrap-{}", claim.spec.operation_id);
        env.extend([json!({"name":"STALWART_RECOVERY_MODE","value":"1"}),json!({"name":"STALWART_RECOVERY_ADMIN","valueFrom":{"secretKeyRef":{"name":secret_name,"key":"administrator"}}})]);
        Some(decode(
            json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":secret_name,"namespace":namespace,"labels":app,"annotations":annotations},"immutable":true,"type":"Opaque","data":{"administrator":STANDARD.encode(format!("admin:{password}"))}}),
        )?)
    } else {
        None
    };
    let phase = match phase {
        MailPhase::Recovery => "bootstrap",
        MailPhase::Configure => "configure",
        MailPhase::Normal => "normal",
    };
    let probe = json!({"httpGet":{"path":"/healthz/live","port":8080},"periodSeconds":3,"timeoutSeconds":2,"failureThreshold":3});
    let deployment = decode(
        json!({"apiVersion":"apps/v1","kind":"Deployment","metadata":metadata,"spec":{"replicas":if claim.spec.action==MailAction::Ensure{1}else{0},"strategy":{"type":"Recreate"},"revisionHistoryLimit":2,"selector":{"matchLabels":{"webdock.dev/app-id":claim.spec.customer_id}},"template":{"metadata":{"labels":pod_labels,"annotations":{"webdock.dev/mail-phase":phase}},"spec":{"nodeSelector":{"kubernetes.io/hostname":config.node},"automountServiceAccountToken":false,"serviceAccountName":"default","terminationGracePeriodSeconds":15,"securityContext":{"runAsNonRoot":true,"runAsUser":65532,"runAsGroup":65532,"fsGroup":65532,"seccompProfile":{"type":"RuntimeDefault"}},"containers":[{"name":"app","image":config.image,"imagePullPolicy":"IfNotPresent","args":["--config","/data/config.json"],"ports":ports,"env":env,"securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},"resources":{"requests":limits,"limits":limits},"readinessProbe":probe,"livenessProbe":probe,"startupProbe":{"httpGet":{"path":"/healthz/live","port":8080},"periodSeconds":3,"timeoutSeconds":2,"failureThreshold":20},"volumeMounts":[{"name":"tmp","mountPath":"/tmp"},{"name":"data","mountPath":"/data"}]}],"volumes":[{"name":"tmp","emptyDir":{"sizeLimit":resource.ephemeral_bytes.to_string()}},{"name":"data","persistentVolumeClaim":{"claimName":pvc}}]}}}}),
    )?;
    let service = decode(
        json!({"apiVersion":"v1","kind":"Service","metadata":metadata,"spec":{"type":"ClusterIP","selector":{"webdock.dev/app-id":claim.spec.customer_id},"ports":[{"name":"http","port":8080,"targetPort":8080,"protocol":"TCP"},{"name":"smtp","port":2525,"targetPort":2525,"protocol":"TCP"},{"name":"submissions","port":2465,"targetPort":2465,"protocol":"TCP"},{"name":"imaps","port":1993,"targetPort":1993,"protocol":"TCP"}]}}),
    )?;
    let volume = decode(
        json!({"apiVersion":"v1","kind":"PersistentVolume","metadata":{"name":pv,"labels":app,"annotations":annotations},"spec":{"capacity":{"storage":resource.volume_bytes.to_string()},"accessModes":["ReadWriteOnce"],"volumeMode":"Filesystem","persistentVolumeReclaimPolicy":"Retain","storageClassName":"","claimRef":{"namespace":namespace,"name":pvc},"local":{"path":storage.root.join(&claim.spec.customer_id).join("mount/data")},"nodeAffinity":{"required":{"nodeSelectorTerms":[{"matchExpressions":[{"key":"kubernetes.io/hostname","operator":"In","values":[storage.node]}]}]}}}}),
    )?;
    let volume_claim = decode(
        json!({"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":{"name":pvc,"namespace":namespace,"labels":app,"annotations":annotations},"spec":{"accessModes":["ReadWriteOnce"],"volumeMode":"Filesystem","storageClassName":"","volumeName":pv,"resources":{"requests":{"storage":resource.volume_bytes.to_string()}}}}),
    )?;
    let namespaced = |name: &str| json!({"name":name,"namespace":namespace,"labels":owner,"annotations":annotations});
    let policies=[("webdock-deny",json!({"podSelector":{},"policyTypes":["Ingress","Egress"],"ingress":[],"egress":[]})),("webdock-internal",json!({"podSelector":{},"policyTypes":["Ingress","Egress"],"ingress":[],"egress":[]})),("webdock-dns",json!({"podSelector":{},"policyTypes":["Egress"],"egress":[{"to":[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":"kube-system"}},"podSelector":{"matchLabels":{"k8s-app":"kube-dns"}}}],"ports":[{"protocol":"UDP","port":53},{"protocol":"TCP","port":53}]}]})),("webdock-ingress",json!({"podSelector":{"matchLabels":{"security.webdock.dev/ingress":"true"}},"policyTypes":["Ingress"],"ingress":[]})),("webdock-mail-gateway",json!({"podSelector":{"matchLabels":{"webdock.dev/mail-customer-id":claim.spec.customer_id}},"policyTypes":["Ingress"],"ingress":[{"from":[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":config.gateway_namespace}},"podSelector":{"matchLabels":{"app":"mail-gateway"}}}],"ports":[{"protocol":"TCP","port":8080}]}]}))].into_iter().map(|(name,spec)|decode(json!({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":namespaced(name),"spec":spec}))).collect::<Result<Vec<_>>>()?;
    Ok(MailObjects {
        namespace: decode(
            json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":namespace,"labels":{ "security.webdock.dev/managed-by":"webdock","webdock.dev/project-id":claim.spec.customer_id,"webdock.dev/cluster-id":claim.spec.cluster_id,"webdock.dev/mail-installation":config.installation,"security.webdock.dev/ready":"true","pod-security.kubernetes.io/enforce":"restricted","pod-security.kubernetes.io/enforce-version":"v1.36"},"annotations":annotations}}),
        )?,
        deployment,
        service,
        volume,
        claim: volume_claim,
        secret,
        quota: decode(
            json!({"apiVersion":"v1","kind":"ResourceQuota","metadata":namespaced("webdock-allocation"),"spec":{"hard":{"requests.cpu":format!("{}m",resource.cpu_millicores),"limits.cpu":format!("{}m",resource.cpu_millicores),"requests.memory":resource.memory_bytes.to_string(),"limits.memory":resource.memory_bytes.to_string(),"requests.ephemeral-storage":resource.ephemeral_bytes.to_string(),"limits.ephemeral-storage":resource.ephemeral_bytes.to_string(),"pods":"10","services":"1","services.nodeports":"0","services.loadbalancers":"0","persistentvolumeclaims":"1","requests.storage":resource.volume_bytes.to_string(),"count/jobs.batch":"0","count/secrets":"20","count/configmaps":"20"}}}),
        )?,
        limits: decode(
            json!({"apiVersion":"v1","kind":"LimitRange","metadata":namespaced("webdock-defaults"),"spec":{"limits":[{"type":"Container","defaultRequest":{"cpu":"100m","memory":"128Mi","ephemeral-storage":"128Mi"},"default":{"cpu":"100m","memory":"128Mi","ephemeral-storage":"128Mi"}}]}}),
        )?,
        account: decode(
            json!({"apiVersion":"v1","kind":"ServiceAccount","metadata":namespaced("default"),"automountServiceAccountToken":false}),
        )?,
        policies,
    })
}
