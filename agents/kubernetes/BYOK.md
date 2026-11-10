# Customer-owned Kubernetes agent

Install on a Linux management host with Python 3 and kubectl. Kubernetes credentials remain local. Extract the bundle, then run the exact command from Webdock and enter the one-time token interactively. The installer creates an enabled systemd service; no workstation connection is needed. Use a dedicated kubeconfig with verified TLS and only the following permissions, scoped to the namespaces enabled by the platform administrator.

Discovery: cluster-scoped get/list nodes, namespaces and storageclasses; get the kube-system Namespace for stable cluster UID. In each allowed namespace: get/list deployments, statefulsets, daemonsets, pods, jobs, cronjobs, services, ingresses, PVCs and HPAs. Optional get/list pods in metrics.k8s.io. No Secrets, ConfigMaps, exec, attach, RBAC writes, node writes or workload create/delete permissions are needed.

Management: namespace-scoped get/patch deployments/scale and statefulsets/scale, patch deployments/statefulsets/daemonsets (restart annotation), patch cronjobs (suspend), get pods/log. Use RoleBindings in each approved namespace. RBAC cannot restrict a patch to a specific annotation; the agent applies only typed, UID/resourceVersion-fenced changes. Give the service account no broader rights than these. Do not use cluster-admin credentials. Kubernetes credential rotation is the cluster owner's responsibility; use your provider's supported renewing credentials. External credential helpers are executed by kubectl on the agent host; only use kubeconfig files you control.

The agent collects allowlisted metadata, never Secret/ConfigMap data or environment values. Logs are requested explicitly and may contain application data. Scans are bounded and identify missing permissions. CPU/RAM metrics need Metrics Server. No registry access, image pull, arbitrary remote shell or YAML deployment is performed.

Operations on recognized GitOps resources and HPA-controlled scaling are blocked. Workloads keep running when disconnected. Revoke the agent in Webdock before replacing it. The registration quota counts revoked clusters too; contact the operator to change the quota. No automatic cluster/data deletion is provided.

Generate the concrete RBAC document for each approved application namespace:

```
python3 permissions.py --namespace apps --namespace production > webdock-rbac.json
kubectl apply -f webdock-rbac.json
```

Only include namespaces explicitly enabled in Webdock. Run these commands as the cluster owner after reviewing the generated document. It creates `spitzli-agent` ServiceAccount and scoped bindings; it does not create application namespaces or change their existing policies. Supply your supported, renewing service-account authentication in the installer kubeconfig; a manually generated short-lived token stops working at expiry. Do not substitute a cluster-admin kubeconfig for missing credential provisioning.
