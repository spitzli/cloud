use super::*;
use futures_util::AsyncReadExt;
use std::collections::BTreeMap;
impl KubernetesAdapter {
    async fn metric_pods(&self, spec: &RuntimeSpec) -> Result<BTreeMap<String, (String, u64)>> {
        let legacy = spec.hosting_binding.is_some();
        let ns = if legacy {
            crate::legacy::namespace(spec)
        } else {
            spec.namespace()
        };
        let labels = if legacy {
            crate::legacy::owner_labels(spec, true)?
        } else {
            spec.labels()
        };
        let selector = labels
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",");
        let api: Api<Pod> = Api::namespaced(self.client.clone(), &ns);
        let pods = api
            .list(&ListParams::default().labels(&selector).limit(201))
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if pods.items.is_empty()
            || pods.items.len() > 200
            || pods.metadata.continue_.is_some_and(|v| !v.is_empty())
        {
            return Err(ExecutionError::Unsupported("complete running pod metrics"));
        }
        let mut result = BTreeMap::new();
        for pod in pods.items {
            if pod.metadata.deletion_timestamp.is_some()
                || pod.status.as_ref().and_then(|s| s.phase.as_deref()) != Some("Running")
                || labels
                    .iter()
                    .any(|(k, v)| pod.metadata.labels.as_ref().and_then(|l| l.get(k)) != Some(v))
            {
                return Err(ExecutionError::Ownership);
            }
            let uid = pod.metadata.uid.ok_or(ExecutionError::Ownership)?;
            let name = pod.metadata.name.ok_or(ExecutionError::Ownership)?;
            let created = pod
                .metadata
                .creation_timestamp
                .ok_or(ExecutionError::Ownership)?
                .0
                .as_second();
            let created = u64::try_from(created).map_err(|_| ExecutionError::Ownership)?;
            if name.is_empty()
                || name.len() > 253
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
                || result.insert(uid, (name, created)).is_some()
            {
                return Err(ExecutionError::Ownership);
            }
        }
        Ok(result)
    }
    pub(super) async fn resource_metrics(&self, spec: &RuntimeSpec) -> Result<ResourceMetrics> {
        self.validate(spec)?;
        let before = self.inspect(spec).await?;
        if before.runtime_id.is_none() {
            return Err(ExecutionError::Ownership);
        }
        let pods = self.metric_pods(spec).await?;
        let ns = if spec.hosting_binding.is_some() {
            crate::legacy::namespace(spec)
        } else {
            spec.namespace()
        };
        let container = if spec.hosting_binding.is_some() {
            "app"
        } else {
            "runtime"
        };
        let mut cpu = 0u64;
        let mut memory = 0u64;
        let mut observed = timestamp()?;
        let mut window: Option<Option<u64>> = None;
        for (uid, (name, created)) in &pods {
            let request = tonic::codegen::http::Request::builder()
                .uri(format!(
                    "/apis/metrics.k8s.io/v1beta1/namespaces/{ns}/pods/{name}"
                ))
                .body(Vec::new())
                .map_err(|_| ExecutionError::Invalid("pod metrics request"))?;
            let work = async {
                let stream =
                    self.client
                        .request_stream(request)
                        .await
                        .map_err(|error| match error {
                            kube::Error::Api(e) if e.code == 404 || e.code == 503 => {
                                ExecutionError::Unsupported("PodMetrics unavailable")
                            }
                            _ => ExecutionError::Runtime,
                        })?;
                let mut stream = std::pin::pin!(stream);
                let mut bytes = Vec::new();
                stream
                    .as_mut()
                    .take(65537)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
                if bytes.len() > 65536 {
                    return Err(ExecutionError::Invalid("pod metrics limit"));
                }
                serde_json::from_slice::<Value>(&bytes)
                    .map_err(|_| ExecutionError::Invalid("pod metrics"))
            };
            let value = tokio::time::timeout(Duration::from_secs(10), work)
                .await
                .map_err(|_| ExecutionError::Expired)??;
            let sample = select_sample(&value, &ns, name, uid, container, *created, timestamp()?)?;
            cpu = cpu
                .checked_add(sample.cpu_nanocores.ok_or(ExecutionError::Runtime)?)
                .ok_or(ExecutionError::Invalid("metrics overflow"))?;
            memory = memory
                .checked_add(sample.memory_bytes.ok_or(ExecutionError::Runtime)?)
                .ok_or(ExecutionError::Invalid("metrics overflow"))?;
            observed = observed.min(sample.observed_at);
            window = Some(match window {
                None => sample.window_nanoseconds,
                Some(previous) if previous == sample.window_nanoseconds => previous,
                _ => None,
            });
        }
        if self.metric_pods(spec).await? != pods
            || self.inspect(spec).await?.runtime_id != before.runtime_id
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(ResourceMetrics {
            source: Some(MetricsSource::KubernetesMetricsApi),
            window_nanoseconds: window.flatten(),
            observed_at: observed,
            cpu_nanoseconds: None,
            cpu_nanocores: Some(cpu),
            memory_bytes: Some(memory),
        })
    }
}
fn select_sample(
    value: &Value,
    namespace: &str,
    name: &str,
    uid: &str,
    container: &str,
    created: u64,
    now: u64,
) -> Result<ResourceMetrics> {
    if value["apiVersion"] != "metrics.k8s.io/v1beta1"
        || value["kind"] != "PodMetrics"
        || value["metadata"]["namespace"] != namespace
        || value["metadata"]["name"] != name
        || value["metadata"]["uid"]
            .as_str()
            .is_some_and(|actual| actual != uid)
    {
        return Err(ExecutionError::Ownership);
    }
    let time = chrono::DateTime::parse_from_rfc3339(
        value["timestamp"]
            .as_str()
            .ok_or(ExecutionError::Invalid("metrics time"))?,
    )
    .map_err(|_| ExecutionError::Invalid("metrics time"))?
    .timestamp();
    let time = u64::try_from(time).map_err(|_| ExecutionError::Invalid("metrics time"))?;
    if time < created || time > now.saturating_add(5) || now.saturating_sub(time) > 120 {
        return Err(ExecutionError::Expired);
    }
    let containers = value["containers"]
        .as_array()
        .ok_or(ExecutionError::Invalid("container metrics"))?;
    let mut selected = containers.iter().filter(|c| c["name"] == container);
    let sample = selected
        .next()
        .ok_or(ExecutionError::Unsupported("container metrics pending"))?;
    if selected.next().is_some() {
        return Err(ExecutionError::Ownership);
    }
    let cpu = super::parse_quantity_scaled(
        sample["usage"]["cpu"]
            .as_str()
            .ok_or(ExecutionError::Invalid("CPU metrics"))?,
        1_000_000_000,
    )?;
    let memory = super::parse_quantity(
        sample["usage"]["memory"]
            .as_str()
            .ok_or(ExecutionError::Invalid("memory metrics"))?,
        false,
    )?;
    let window = value["window"]
        .as_str()
        .filter(|v| v.len() <= 64)
        .ok_or(ExecutionError::Invalid("metrics window"))?;
    let window =
        humantime::parse_duration(window).map_err(|_| ExecutionError::Invalid("metrics window"))?;
    if window.is_zero() || window > Duration::from_secs(300) {
        return Err(ExecutionError::Invalid("metrics window"));
    }
    Ok(ResourceMetrics {
        source: Some(MetricsSource::KubernetesMetricsApi),
        window_nanoseconds: Some(
            u64::try_from(window.as_nanos())
                .map_err(|_| ExecutionError::Invalid("metrics window"))?,
        ),
        observed_at: time,
        cpu_nanoseconds: None,
        cpu_nanocores: Some(cpu),
        memory_bytes: Some(memory),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn namespace_metrics_report_cpu_rate_and_refuse_stale_or_foreign_pods() {
        let sample = json!({"apiVersion":"metrics.k8s.io/v1beta1","kind":"PodMetrics","metadata":{"namespace":"owned","name":"pod","uid":"exact"},"timestamp":"2026-01-01T00:00:00Z","window":"15s","containers":[{"name":"runtime","usage":{"cpu":"123456n","memory":"123Ki"}}]});
        let now = 1767225600;
        let result =
            select_sample(&sample, "owned", "pod", "exact", "runtime", now - 10, now).unwrap();
        assert_eq!(result.cpu_nanoseconds, None);
        assert_eq!(result.cpu_nanocores, Some(123456));
        assert_eq!(result.memory_bytes, Some(125952));
        assert!(
            select_sample(
                &sample,
                "owned",
                "pod",
                "different",
                "runtime",
                now - 10,
                now
            )
            .is_err()
        );
        assert!(
            select_sample(
                &sample,
                "owned",
                "pod",
                "exact",
                "runtime",
                now - 10,
                now + 121
            )
            .is_err()
        );
        assert!(
            select_sample(
                &sample,
                "owned",
                "pod",
                "exact",
                "runtime",
                now + 1,
                now + 1
            )
            .is_err()
        );
    }
    #[test]
    fn metrics_permission_excludes_node_proxy_and_exec() {
        let role: Value =
            serde_saphyr::from_str(include_str!("../../deploy/metrics-role.yaml")).unwrap();
        assert_eq!(role["kind"], "Role");
        assert_eq!(
            role["rules"],
            json!([{"apiGroups":["metrics.k8s.io"],"resources":["pods"],"verbs":["get"]}])
        );
    }
}
