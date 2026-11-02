# Plugin workloads (step 3) Implementation Plan

> Executed inline. Spec: `docs/superpowers/specs/2026-10-21-product-plugins-design.md` ("Infrastructure access"). Owner (2026-10-22): k3s with its bundled Traefik for routing; after this, domains/TLS and Git builds for hosting.

**Goal:** Plugins run their workloads (hosting apps, dedicated PostgreSQL, mail engines) on a cluster through its enrolled agent instead of a local Docker placement, and apps get a domain through Traefik.

**Architecture:** A plugin submits desired Kubernetes objects for one unit (usually one resource) with the host capability `plugin.workloads`. Core validates them against what the manifest declares and the operator approved, pins them to a Core-chosen namespace with owner labels, and stores them. The cluster's agent polls, applies them with server-side apply, prunes objects that left the unit, and reports observed state, which the plugin reads back. The agent knows nothing about PostgreSQL, mail or apps.

## Rulings (my calls)

- Workload calls use the plugin's runtime identity (peer + runtime credential), not an invocation token. Provisioning outlasts the 60-second invocation, and the plugin already authenticates as its runtime. Every call names a project; Core checks that the project exists and that the unit belongs to this installation. Cost if wrong: a compromised plugin process can touch the workloads of its own installation in any project, which it can already reach through its own database.
- Namespace per installation and project: `wd-<plugin>-<project>`. Plugins cannot choose namespaces or cluster-scoped kinds.
- Allowed kinds come from the manifest (`workloads.kinds`); Core additionally refuses anything outside a fixed safe set (Deployment, StatefulSet, Service, Secret, ConfigMap, PersistentVolumeClaim, NetworkPolicy, Ingress, traefik IngressRoute/Middleware, postgresql.cnpg.io Cluster/Pooler). No Pods with hostPath, hostNetwork, privileged or added capabilities.
- One local cluster in dev: k3s in Docker Compose (bundled Traefik), an enrolled agent over a private Unix socket. Production clusters enroll the same way over mTLS.
- Cluster choice: the installation configuration names the default cluster; per-project placement comes later.

## Tasks

1. **Dev: local k3s.** Compose service with Traefik, kubeconfig in `.dev-local/k3s`, ports on loopback. Tilt resource.
2. **Protocol:** manifest `workloads` declaration; host capability `plugin.workloads` (`apply`, `observe`, `remove`); runtime-authority host calls.
3. **Core:** `plugin_workload` table (desired objects, revision, observed state), validation and labelling, `WorkloadControl` gRPC service for agents (poll, report), audit.
4. **Agent:** generic workload loop: server-side apply of allowlisted kinds, prune by unit label, readiness observation, report.
5. **Dev: local agent** enrolled for the local cluster in the seed and run by Tilt.
6. **Hosting plugin on Kubernetes:** Deployment, Service, Secret for environment, Traefik route `<app>.<workspace>.apps.localhost`. Docker placement removed.
7. **Dedicated PostgreSQL on CNPG** and **mail engines on Kubernetes** (CNPG operator in local k3s; mail with an egress-deny NetworkPolicy).
8. Live run, docs, push.

## Status (2026-10-22)

All tasks are done and were run live on the local stack (Tilt, k3s, enrolled agent):

- Hosting: apps answer on `http://<name>-<id>.apps.localhost:55480`; logs, environment, stop/start and delete go through the unit.
- Dedicated PostgreSQL: CloudNativePG 0.29.1 (chart, installed by k3s), one `Cluster` per instance, reached on `pg-<id>.db.localhost:55442` with `sslmode=require`.
- Mail: one Stalwart engine per project with a deny-all-egress NetworkPolicy; IMAPS on 55993 and submission on 55465, by server name.
- Plugins share the host call through `webdock_plugin_kit::workloads`.

Rulings made while building:

- Database and mail clients are routed through Traefik TCP routers by TLS server name with passthrough (PostgreSQL via Traefik's STARTTLS support). Certificates come from cert-manager (`Certificate` is a workload kind; Core refuses `isCA`). Local dev issues from its own CA, limited to `*.localhost` names (`bun tools/dev/local.mjs cluster-ca`); production swaps the ClusterIssuer for an ACME one. Cost if wrong: Stalwart reads its certificate files at load, so a renewed certificate takes effect after an engine restart.
- Dedicated passwords are declarative: the role's Secret carries `cnpg.io/reload` and CloudNativePG's managed roles apply a changed password within seconds, so a rotation is just a new apply and nothing is staged. Cost if wrong: for a few seconds after a rotation the shown password may not work yet.
- The plugin manages mail engines over Stalwart's HTTP API through an IngressRoute on the HTTP entry point, protected by the engine's admin password. Cost if wrong: in production this must move to an internal entry point, or the plugin must run in the cluster.
- Core still accepts any host in plugin routes. Before more than one installation shares a Traefik, Core should pin route hosts to each plugin's configured domain.
