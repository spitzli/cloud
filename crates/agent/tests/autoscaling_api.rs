#![cfg(feature = "kubernetes")]
mod support;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Mutex,
};
use webdock_agent::kubernetes::autoscaling;
use webdock_supervisor::*;

struct Allow;
#[async_trait::async_trait]
impl Authorizer for Allow {
    async fn authorize(&self, _: &Command) -> Result<()> {
        Ok(())
    }
}
struct Server {
    hpa: Option<Value>,
    deployment: Value,
    writes: Vec<Value>,
}
async fn server(
    spec: &RuntimeSpec,
) -> (
    kube::Client,
    Arc<Mutex<Server>>,
    tokio::task::JoinHandle<()>,
) {
    let mut hpa = autoscaling::manifest(spec, "owned-uid").unwrap();
    hpa["metadata"]["uid"] = json!("hpa-uid");
    hpa["metadata"]["resourceVersion"] = json!("3");
    hpa["status"] = json!({"desiredReplicas":2,"conditions":[{"type":"ScalingActive","status":"False","reason":"FailedGetResourceMetric"}]});
    let state = Arc::new(Mutex::new(Server {
        hpa: Some(hpa),
        deployment: json!({"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":spec.name(),"namespace":spec.namespace(),"uid":"owned-uid","resourceVersion":"7"},"spec":{"replicas":2,"selector":{"matchLabels":{}},"template":{"metadata":{},"spec":{"containers":[]}}},"status":{"replicas":2,"availableReplicas":2}}),
        writes: vec![],
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let shared = state.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let state = shared.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 4096];
                let header_end = loop {
                    let n = socket.read(&mut buffer).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                    assert!(bytes.len() < 65536);
                    if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let header = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
                let length = header
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|n| n.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while bytes.len() < header_end + length {
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let mut request = header.lines().next().unwrap().split_whitespace();
                let method = request.next().unwrap();
                let path = request.next().unwrap();
                let mut state = state.lock().await;
                let (code, value) = if method == "GET" && path.contains("horizontalpodautoscalers")
                {
                    state.hpa.clone().map(|h|(200,h)).unwrap_or((404,json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"NotFound","code":404})))
                } else if method == "GET" && path.contains("deployments") {
                    (200, state.deployment.clone())
                } else if method == "PUT" && path.contains("horizontalpodautoscalers") {
                    let mut body: Value =
                        serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                    if state.hpa.as_ref().unwrap()["metadata"]["resourceVersion"]
                        != body["metadata"]["resourceVersion"]
                    {
                        (
                            409,
                            json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"Conflict","code":409}),
                        )
                    } else {
                        let next = body["metadata"]["resourceVersion"]
                            .as_str()
                            .unwrap()
                            .parse::<u64>()
                            .unwrap()
                            + 1;
                        body["metadata"]["resourceVersion"] = json!(next.to_string());
                        state.writes.push(body.clone());
                        state.hpa = Some(body.clone());
                        (200, body)
                    }
                } else if method == "DELETE" && path.contains("horizontalpodautoscalers") {
                    let body: Value =
                        serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                    state.writes.push(body);
                    state.hpa = None;
                    (
                        200,
                        json!({"apiVersion":"v1","kind":"Status","status":"Success"}),
                    )
                } else {
                    panic!("Unexpected fixture call {method} {path}")
                };
                let body = value.to_string();
                let reason = if code == 200 { "OK" } else { "Not Found" };
                socket.write_all(format!("HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            });
        }
    });
    let config = kube::Config::new(format!("http://{address}").parse().unwrap());
    (kube::Client::try_from(config).unwrap(), state, task)
}
fn spec() -> RuntimeSpec {
    let mut s = support::spec();
    s.autoscaling = Some(AutoscalingPolicy {
        min_replicas: 1,
        max_replicas: 4,
        target_cpu_utilization_percent: 70,
    });
    s
}
fn command(spec: RuntimeSpec) -> Command {
    Command {
        operation_id: "owned-stop-test".into(),
        generation: 1,
        lease_until: timestamp().unwrap() + 60,
        authorization: "test".into(),
        material: None,
        action: Action::Stop,
        spec,
        expected_runtime_id: Some("owned-uid".into()),
    }
}

#[tokio::test]
async fn removal_is_conditioned_on_exact_custody_and_live_authority() {
    let spec = spec();
    let (client, state, task) = server(&spec).await;
    let cmd = command(spec.clone());
    state.lock().await.hpa.as_mut().unwrap()["metadata"]["ownerReferences"][0]["uid"] =
        json!("foreign");
    assert!(
        autoscaling::remove(
            client.clone(),
            &cmd,
            &LeaseGuard::new(cmd.clone(), Arc::new(Allow))
        )
        .await
        .is_err()
    );
    assert!(state.lock().await.writes.is_empty());
    state.lock().await.hpa.as_mut().unwrap()["metadata"]["ownerReferences"][0]["uid"] =
        json!("owned-uid");
    let mut expired = cmd.clone();
    expired.lease_until = timestamp().unwrap() - 1;
    assert!(
        autoscaling::remove(
            client.clone(),
            &expired,
            &LeaseGuard::new(expired.clone(), Arc::new(Allow))
        )
        .await
        .is_err()
    );
    assert!(state.lock().await.writes.is_empty());
    autoscaling::remove(
        client.clone(),
        &cmd,
        &LeaseGuard::new(cmd.clone(), Arc::new(Allow)),
    )
    .await
    .unwrap();
    let state = state.lock().await;
    assert_eq!(
        state.writes[0]["preconditions"],
        json!({"uid":"hpa-uid","resourceVersion":"3"})
    );
    assert!(state.hpa.is_none());
    task.abort();
}

#[tokio::test]
async fn missing_metrics_preserves_observed_counts_without_mutation() {
    let spec = spec();
    let (client, state, task) = server(&spec).await;
    let mut observed:ObservedState=serde_json::from_value(json!({"resourceId":"456","revision":1,"phase":"running","ready":true,"observedAt":timestamp().unwrap(),"runtimeId":"owned-uid","conditions":[]})).unwrap();
    autoscaling::observe(client.clone(), &spec, &mut observed)
        .await
        .unwrap();
    let observation = observed.autoscaling.unwrap();
    assert!(!observation.metrics_available);
    assert_eq!(observation.current_replicas, 2);
    assert_eq!(observation.ready_replicas, 2);
    assert!(state.lock().await.writes.is_empty());
    assert!(autoscaling::ensure_absent(client, &spec).await.is_err());
    task.abort();
}

#[tokio::test]
async fn running_count_is_observed_not_the_requested_deployment_size() {
    let spec = spec();
    let (client, state, task) = server(&spec).await;
    {
        let mut state = state.lock().await;
        state.deployment["spec"]["replicas"] = json!(4);
        state.deployment["status"]["replicas"] = json!(1);
        state.deployment["status"]["availableReplicas"] = json!(1);
        state.hpa.as_mut().unwrap()["status"]["desiredReplicas"] = json!(4);
    }
    let mut observed: ObservedState = serde_json::from_value(json!({"resourceId":"456","revision":1,"phase":"starting","ready":false,"observedAt":timestamp().unwrap(),"runtimeId":"owned-uid","conditions":[]})).unwrap();
    autoscaling::observe(client, &spec, &mut observed)
        .await
        .unwrap();
    let scaling = observed.autoscaling.unwrap();
    assert_eq!(scaling.current_replicas, 1);
    assert_eq!(scaling.ready_replicas, 1);
    assert_eq!(scaling.desired_replicas, 4);
    task.abort();
}

#[tokio::test]
#[ignore = "isolated process with WEBDOCK_AGENT_AUTOSCALING_ENABLED=true"]
async fn unchanged_policy_fences_delayed_older_replacement() {
    assert!(autoscaling::enabled());
    let mut spec = spec();
    let (client, state, task) = server(&spec).await;
    let mut delayed = state.lock().await.hpa.clone().unwrap();
    delayed["metadata"]["annotations"]["spitzli.cloud/scaling-revision"] = json!("2");
    delayed["spec"]["maxReplicas"] = json!(2);
    spec.revision = 3;
    let mut cmd = command(spec.clone());
    cmd.action = Action::Apply;
    autoscaling::apply(
        client.clone(),
        &cmd,
        &LeaseGuard::new(cmd.clone(), Arc::new(Allow)),
        "owned-uid",
    )
    .await
    .unwrap();
    let api: kube::Api<k8s_openapi::api::autoscaling::v2::HorizontalPodAutoscaler> =
        kube::Api::namespaced(client, &spec.namespace());
    let delayed = serde_json::from_value(delayed).unwrap();
    assert!(
        matches!(api.replace(&spec.name(),&kube::api::PostParams::default(),&delayed).await,Err(kube::Error::Api(error)) if error.code==409)
    );
    let state = state.lock().await;
    let current = state.hpa.as_ref().unwrap();
    assert_eq!(
        current["metadata"]["annotations"]["spitzli.cloud/scaling-revision"],
        "3"
    );
    assert_eq!(current["spec"]["maxReplicas"], 4);
    assert_eq!(state.writes.len(), 1);
    task.abort();
}
