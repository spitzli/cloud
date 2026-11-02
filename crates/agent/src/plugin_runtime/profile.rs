use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

/// No file names, paths or shell fragments come from dispatch material.
pub const STAGE_SCRIPT: &str = "umask 077; mkdir -p /private/runtime; chmod 700 /private/runtime; for file in runtime.json manifest.json runtime.crt runtime.key core-ca.crt storage-ca.crt; do cp /source/$file /private/runtime/$file; chmod 600 /private/runtime/$file; done";

/// Fixed native-config profile. The only executable is the reviewed digest artifact.
pub fn plugin_objects(claim: &PluginRuntimeClaim) -> Result<Vec<Value>> {
    claim
        .validate(timestamp()?)
        .map_err(|_| ExecutionError::Denied)?;
    let s = &claim.spec;
    let labels = s.labels();
    let hash = s.fingerprint().map_err(|_| ExecutionError::Denied)?;
    let annotations = json!({"webdock.dev/spec-sha256":hash});
    let meta = |name: &str| json!({"name":name,"namespace":s.namespace,"labels":labels,"annotations":annotations});
    let limits = json!({"cpu":format!("{}m",s.cpu_millicores),"memory":s.memory_bytes.to_string(),"ephemeral-storage":"16Mi"});
    let security = json!({"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}});
    let secret_data = claim
        .material
        .files
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(STANDARD.encode(v))))
        .collect::<serde_json::Map<_, _>>();
    let replicas = if claim.lease.action == PluginRuntimeAction::Suspend {
        0
    } else {
        1
    };
    let peers = json!([{"namespaceSelector":{"matchLabels":{"webdock.dev/role":"control-plane"}},"podSelector":{"matchLabels":{"webdock.dev/component":"control-plane"}}},{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":"kube-system"}},"podSelector":{"matchLabels":{"app.kubernetes.io/name":"traefik"}}}]);
    let mut egress=s.egress.iter().map(|p|json!({"to":[{"ipBlock":{"cidr":p.cidr}}],"ports":[{"protocol":"TCP","port":p.port}]})).collect::<Vec<_>>();
    egress.push(json!({"to":[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":"kube-system"}},"podSelector":{"matchLabels":{"k8s-app":"kube-dns"}}}],"ports":[{"protocol":"UDP","port":53},{"protocol":"TCP","port":53}]}));
    let aliases = s
        .host_aliases
        .iter()
        .map(|a| json!({"ip":a.ip,"hostnames":a.hostnames}))
        .collect::<Vec<_>>();
    let initcontainers = json!([{"name":"private-config","image":s.image,"imagePullPolicy":"IfNotPresent","command":["/bin/sh","-ec",STAGE_SCRIPT],"securityContext":security,"resources":{"requests":limits,"limits":limits},"terminationMessagePath":"/dev/termination-log","terminationMessagePolicy":"File","volumeMounts":[{"name":"source","mountPath":"/source","readOnly":true},{"name":"private","mountPath":"/private"}]}]);
    let containers = json!([{"name":"runtime","image":s.image,"imagePullPolicy":"IfNotPresent","args":["/run/webdock/runtime/runtime.json"],"ports":[{"name":"runtime","containerPort":s.port,"protocol":"TCP"}],"securityContext":security,"resources":{"requests":limits,"limits":limits},"terminationMessagePath":"/dev/termination-log","terminationMessagePolicy":"File","readinessProbe":{"tcpSocket":{"port":"runtime"},"initialDelaySeconds":1,"periodSeconds":2,"timeoutSeconds":1,"successThreshold":1,"failureThreshold":3},"volumeMounts":[{"name":"private","mountPath":"/run/webdock","readOnly":true},{"name":"temporary","mountPath":"/tmp"}]}]);
    let volumes = json!([{"name":"source","secret":{"secretName":"runtime-config","defaultMode":288,"optional":false,"items":MATERIAL_FILES.iter().map(|f|json!({"key":f,"path":f,"mode":288})).collect::<Vec<_>>()}},{"name":"private","emptyDir":{"medium":"Memory","sizeLimit":"1Mi"}},{"name":"temporary","emptyDir":{"medium":"Memory","sizeLimit":"16Mi"}}]);
    let mut namespace_labels = labels.clone();
    namespace_labels.insert(
        "pod-security.kubernetes.io/enforce".into(),
        "restricted".into(),
    );
    Ok(vec![
        json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":s.namespace,"labels":namespace_labels,"annotations":annotations}}),
        json!({"apiVersion":"v1","kind":"Secret","metadata":meta("runtime-config"),"type":"Opaque","immutable":true,"data":secret_data}),
        json!({"apiVersion":"apps/v1","kind":"Deployment","metadata":meta(&s.service),"spec":{"replicas":replicas,"strategy":{"type":"Recreate"},"revisionHistoryLimit":2,"progressDeadlineSeconds":120,"selector":{"matchLabels":labels},"template":{"metadata":{"labels":labels,"annotations":annotations},"spec":{
            "serviceAccountName":"default","automountServiceAccountToken":false,"enableServiceLinks":false,"hostAliases":aliases,"dnsPolicy":"ClusterFirst","restartPolicy":"Always","schedulerName":"default-scheduler","terminationGracePeriodSeconds":20,
            "securityContext":{"runAsNonRoot":true,"runAsUser":65532,"runAsGroup":65532,"fsGroup":65532,"fsGroupChangePolicy":"OnRootMismatch","seccompProfile":{"type":"RuntimeDefault"}},
            "initContainers":initcontainers,
            "containers":containers,
            "volumes":volumes
        }}}}),
        json!({"apiVersion":"v1","kind":"Service","metadata":meta(&s.service),"spec":{"type":"ClusterIP","selector":labels,"ports":[{"name":"runtime","port":s.port,"targetPort":"runtime","protocol":"TCP"}]}}),
        json!({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":meta("runtime"),"spec":{"podSelector":{"matchLabels":labels},"policyTypes":["Ingress","Egress"],"ingress":[{"from":peers,"ports":[{"protocol":"TCP","port":s.port}]}],"egress":egress}}),
        json!({"apiVersion":"traefik.io/v1alpha1","kind":"IngressRouteTCP","metadata":meta("runtime"),"spec":{"entryPoints":[s.entry_point],"routes":[{"match":format!("HostSNI(`{}`)",s.runtime_host),"services":[{"name":s.service,"port":s.port}]}],"tls":{"passthrough":true}}}),
    ])
}
