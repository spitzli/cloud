# Native application autoscaling

Automatic scaling is opt-in. It is disabled by default in Core and the agent.
Do not enable it until the target cluster passes the qualification below.
Existing applications retain fixed-instance behavior.

## Policy and capacity

An application spec may include:

```json
{
  "autoscaling": {
    "minReplicas": 1,
    "maxReplicas": 3,
    "targetCPUUtilizationPercent": 70
  }
}
```

Minimum and maximum are integers from 1 to 20; minimum cannot exceed maximum.
Plan and cluster limits can reduce the usable maximum. CPU targets are 10–90%
of the per-instance CPU request. CPU, memory, ephemeral storage and replica
capacity are reserved for the maximum while the application is active. Reserved
capacity is not a measurement of running usage or a new billing policy.

The persisted `replicas` remains lifecycle intent: zero is stopped. A running
automatic spec uses a value inside its policy range; actual instance counts come
from runtime observations. Stop preserves the policy but releases compute after
confirmed shutdown. Start reactivates the policy. Switch to fixed mode explicitly
before manually selecting a count. Changing settings on a stopped app keeps it
stopped.

Local persistent-volume workloads, Docker executors and internal plugin runtimes
do not support this policy. Applications must tolerate concurrent processes,
externalize shared state and coordinate background jobs themselves. In
particular, duplicating a Discord bot process is not automatically safe.

## Runtime and permissions

The agent owns a namespaced `autoscaling/v2` HPA tied to the exact Deployment UID.
The HPA uses CPU metrics, doubles at most once per minute, and applies a 300-second
downscale stabilization window with a maximum 50% decrease per minute. Missing
metrics is shown as unavailable, never as zero load.

For Helm-managed apps, active automatic rendering omits `spec.replicas`. The
retained Deployment adapter preserves the HPA's current count using the existing
resource-version update; it seeds the minimum only on resume from zero. Existing
Recreate rollout strategy is unchanged; autoscaling does not promise zero-downtime
deployments.

Stop, fixed-mode transitions and deletion remove the exact owned HPA before the
final workload mutation. UID/resource-version preconditions prevent deleting a
replacement object. Completion must observe the HPA absent for stopped/fixed
workloads. Uncertain outcomes retain existing reconciliation and reservation
rules; do not manually clear their operation records.

The qualified agent advertises `autoscalingVersion: 1`. Core refuses admission,
dispatch and continued authority when the required capability is unavailable.
An older agent must never interpret an automatic policy as fixed replicas.

## Qualification and rollout

1. Verify an operational `metrics.k8s.io` API and per-pod CPU metrics on the target
   cluster. Review namespace quotas and per-instance resource requests.
2. Run the owned PostgreSQL admission test and the real Kubernetes tests below.
   The latter are restricted to explicitly selected loopback test clusters.
3. Roll out compatible agents with the reviewed HPA permissions limited to their
   application namespaces. `deploy/helm/spitzli-agent` keeps mutation permissions
   disabled unless `autoscalingEnabled` is true. No build-pod permissions change.
4. Set `WEBDOCK_AGENT_AUTOSCALING_ENABLED=true` only on a qualified agent. For the
   Helm agent chart this is controlled by `autoscalingEnabled`.
5. Roll out compatible Core/Console, then set
   `WEBDOCK_HOSTING_AUTOSCALING_ENABLED=true`. Console enables automatic mode only
   where Core reports it available. Existing fixed apps remain unchanged.
6. Test one explicitly opted-in stateless application, record up/down transitions,
   stopped-state durability, current counts and successful fixed-mode transition.
   Never use a customer application as a synthetic load generator.

To disable new admissions, disable the Core flag first. Existing automatic apps
must be transitioned to fixed mode or stopped with compatible agents before
downgrading those agents. Retain the new agent until all owned HPAs are removed.
Stopping retains the saved policy: before downgrading Core to a version without
this contract, explicitly convert those saved configurations to fixed mode too.
Do not roll back to components that cannot manage the remaining policies.

## Local tests

```sh
bun test packages/hosting-contracts/src
bun test apps/console/tests/hosting-autoscaling.test.ts
cargo test --locked -p spitzli-supervisor --test autoscaling
cargo test --locked -p spitzli-hosting --test autoscaling
WEBDOCK_HOSTING_AUTOSCALING_ENABLED=true cargo test --locked \
  -p spitzli-hosting --test autoscaling_postgres -- --ignored
WEBDOCK_AGENT_AUTOSCALING_ENABLED=true cargo test --locked -p spitzli-agent --features kubernetes --test autoscaling_api -- --include-ignored
bun apps/console/tests/autoscaling-browser.mjs
```

The PostgreSQL test creates and removes its own PostgreSQL 18 fixture. It accepts
no external DSN. Real HPA tests additionally require an owned loopback Kubernetes
context named `kind-webdock-rust-*`, `WEBDOCK_KUBE_TEST_CONFIG`,
`WEBDOCK_KUBE_TEST_CONTEXT`, a digest-pinned `WEBDOCK_KUBE_TEST_IMAGE` containing
BusyBox and `WEBDOCK_AGENT_AUTOSCALING_ENABLED=true`:

```sh
cargo test --locked -p spitzli-agent --features kubernetes \
  --test autoscaling real_ -- --ignored --nocapture
```

Those tests create uniquely named application namespaces and a separate restricted
probe namespace with the existing control-plane labels. This represents the
already allowed control traffic. The Helm path also tests refusal of the
unapproved application-origin request with a bounded timeout. The tests do not
relax application network policy to make traffic pass.
