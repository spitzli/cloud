# Application autoscaling

Status: owner-approved design; implementation under qualification, disabled by default.

## Owner requirement

Applications scale automatically up and down with load, within customer-selected
minimum and maximum instance counts. Keep the existing manual scaling mode.

## Existing system

`HostingApplication.tsx` exposes manual `apps.scale`. Native application specs
allow 0–20 replicas. `hosting::contracts::app_demand` accounts for resources per
replica. `hosting::apps` admits operations against customer and cluster capacity.
The Kubernetes agent renders the approved `webdock-plugin` chart, including a
Deployment and Service. It verifies workload identity and waits for rollout.
Persistent local volumes intentionally support one replica only.

There is no implemented HPA writer. Agent RBAC currently only reads HPAs. The
Deployment chart always writes replicas and uses Recreate. Adding an HPA alone
would leave competing replica writers, incomplete reservations and an incomplete
stop/recovery contract. Those must be resolved together.

## Product behavior

Implementation defaults below are engineering choices, not additional owner
requirements:

- Two modes: fixed instances (existing behavior) and automatic instances.
- Automatic policy: `minReplicas`, `maxReplicas`, `targetCPUUtilizationPercent`.
  Require `1 <= min <= max <= 20`; validate CPU target 10–90%; initial UI target 70%.
  Existing plan and capacity limits can impose a smaller maximum.
- CPU is the initial load metric, relative to CPU requests. Request-count,
  queue-depth, memory and scale-to-zero are outside this increment.
- Use Kubernetes `autoscaling/v2` HPA, with a five-minute downscale stabilization
  window and explicit bounded scale-up/down policies. The HPA must target only
  this application's Deployment in its own namespace.
- Keep a policy separate from the observed count. Show mode, min/max, current and
  ready instances, metric availability and an understandable scaling condition.
  Do not show reserved capacity as measured consumption.
- Enabling automatic mode is explicit. No existing app is automatically converted.
  Reject automatic mode on Docker runtimes, plugin-managed internal workloads,
  local persistent-volume apps, and clusters without qualified support/metrics.
- Disable manual `apps.scale` while automatic mode is active; explain how to
  switch to fixed mode. An explicit switch to fixed mode includes its target.
- Stop retains the policy but removes/suspends its execution and scales to zero.
  Start restores it. No autonomous operation may restart a stopped app.

## Capacity, authority and persistence

Extend the existing strict application spec with an optional `autoscaling`
policy, absent for existing apps. Use existing JSON spec persistence; do not add a
native migration merely for this field. All API, TypeScript, runtime protocol,
hashing and validation paths must agree on the shape.

Reserve CPU, memory, ephemeral storage and replica allowance at **maxReplicas**
while automatic mode is active. This deliberately guarantees admitted headroom;
it is not a claim that all reserved instances are currently billed or running.
Existing transactional reservations remain the admission authority. Changing the
maximum, resource sizes or cluster must re-admit the complete bound. Stop/delete
release capacity only through existing confirmed lifecycle semantics.

Extend the signed native runtime contract and add a versioned cluster capability.
Old agents must reject automatic policies, including claim/reconciliation paths;
absence of capability must never silently fall back to a fixed count. Preserve
tenant authorization, operation revision/idempotency, lease fencing, namespace
quotas, recovery guards and immutable source/image authority.

## Kubernetes lifecycle

The HPA is the only dynamic replica writer while automatic mode is active. Omit
`Deployment.spec.replicas` from active automatic-mode rendering, and test the
transition from an existing fixed-mode Helm release. Fixed and stopped rendering
still supplies a count. Do not change all workloads' rollout strategy in this
feature: retain the existing policy and document its interruption behavior.

The agent owns the HPA directly so it can bind the real Deployment UID; Helm
continues to own its existing workload resources. Include the HPA in exact
ownership checks and verify its absence during retirement. Do not adopt it into
the Helm release. Allow only the application's own scale target,
policy and metrics; do not accept arbitrary Kubernetes objects from customers.
Namespace-scoped agent privileges must cover required HPA lifecycle operations;
build jobs receive no new permission.

On stop, delete the owned HPA before the final zero-replica write. Confirm its
absence and zero remaining Pods through the existing fenced completion path.
Exercise a delayed in-flight HPA scale request in tests; an uncertain stop stays
unconfirmed and keeps resources/reservations. On resume, recreate the policy only
under the fresh admitted operation. Recovery/suspend must use the same sequence,
not a separate shortcut that leaves an HPA able to recreate Pods.

Autoscaled readiness and completion must verify the signed policy and owned HPA,
not compare the observed count with a stale fixed spec. Distinguish initial
rollout readiness from later scaling progress. Missing/stale metrics is a visible
degraded scaling condition; never invent utilization or infer idle and scale down.
Keep state during unavailable metrics and let Kubernetes' HPA rules apply.

## Verification and release gates

1. Contract parity: valid/invalid ranges, no extra keys, backward-compatible fixed
   specs, persistent-volume rejection, stopped policy retention.
2. Restricted PostgreSQL: max-bound reservations, tenant denial, concurrent
   competing admissions, stale revision, insufficient capacity and idempotency.
3. Agent/chart: exact HPA identity/target/metrics, ownership refusal, old-agent
   capability refusal, no static replica writer in automatic mode.
4. Owned disposable Kubernetes cluster with working metrics: real CPU workload
   causes scale-up to the bound and later scale-down no lower than the minimum;
   prove requests reach multiple ready instances. Record actual versions.
5. Exercise missing metrics, stop during scale-up, stale HPA write, resume,
   fixed/auto transitions, rollout, rollback, delete and recovery suspension.
   No failed/uncertain effect may report success or release capacity early.
6. Customer UI: fixed/automatic forms, validation, permission denial, unavailable
   capability, current/ready counts and degraded conditions; keyboard/mobile.
7. Release agents/capability and Core/Console in a compatible order, behind a
   disabled-by-default feature gate. Enable only after the live owned-cluster
   qualification. Do not use Lunares or another existing customer app as a load
   generator, and do not promise that a Discord bot supports concurrent copies.

## Reference

Official Kubernetes HPA behavior, metric requirements, stability and replica
management: https://kubernetes.io/docs/concepts/workloads/autoscaling/horizontal-pod-autoscale/
Rechecked 2026-11-08. Public builds and unrelated pending native-Git work remain
unchanged. This feature adds no paid infrastructure.
