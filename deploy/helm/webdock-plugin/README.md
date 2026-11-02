# Webdock runtime chart

This chart is consumed only through a reviewed `ApprovedChart` digest and typed runtime values. Each owned workload receives an independent release in its dedicated namespace. Helm manages the specification; the Kubernetes Deployment controller manages Pods.

There is no public ingress, cluster-wide RBAC, hostPath, privileged container, workload service-account token or arbitrary values-driven manifest. PVCs carry `helm.sh/resource-policy: keep`; suspending through the agent sets replicas to zero and retains storage. Deletion is a separate future workflow.

NetworkPolicy permits the control-plane workload (`webdock.dev/component=control-plane`) in the control-plane namespace (`webdock.dev/role=control-plane`) to reach the runtime port, permits the workload to use Core's HTTPS broker on port 443, and permits cluster DNS. Verify that the selected CNI enforces these policies. The policy does not implement provider FQDN/path rules or replace workload mTLS.

The agent supplies all values after manifest/ownership/lease admission; values.schema.json bounds the chart input as a second check. Generated values, certificate/secret delivery and approved StorageClass/placement policy must be integrated with the installation's existing security and storage configuration before deployment. No production rollout has been performed.
