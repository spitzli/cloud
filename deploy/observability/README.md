# Offline observability foundation

**Live continuation, 2026-10-30:** the dedicated OBS stack is now running with
fixed local volumes and scoped Wasabi storage. Owner-requested operator access is
available at `https://grafana.webdock.dev` and `https://prometheus.webdock.dev`.
See [current access and operating instructions](../../docs/operations/observability-access.md).
The historical offline baseline and its unconfigured default inputs below remain
closed unless combined with the reviewed live overlays. SSO/MFA, customer access,
fleet collection and complete recovery qualification are not delivered.

Activation is disabled. This directory creates no cluster, Secret, bucket, DNS,
ingress, notification destination or paid resource. It contains upstream chart
inputs and an offline qualification helper, with no install/apply command. The
placement and operating proposal remains
[observability-foundation.md](../../docs/architecture/observability-foundation.md).
The separate OBS server is independent of BUILD/RUN. After this offline
qualification the owner authorized a 12-month VPS6 EU purchase; see the
[host/order record](../../docs/operations/observability-host.md). No workload runtime
capacity is demonstrated by purchasing it.

The owner asked to reuse maintained Grafana dashboards and panels. The pinned
kube-prometheus-stack supplies 17 default dashboard ConfigMaps and upstream alert
rules. Their definitions remain inside the verified chart archive; there is no
custom dashboard or chart project. Several panels/rules have no data until the
corresponding reviewed collectors are enabled. Do not interpret an empty panel as
healthy infrastructure. Grafana is the future operator UI, not a shared customer
datasource or embedded anonymous panel.

## Pins and local verification

Checked against the official repository indexes on 2026-10-30:

| Chart | Version / application | Archive SHA256 |
| --- | --- | --- |
| kube-prometheus-stack | 92.16.1 / operator v0.94.2 | `d2af1fccbd11fedad9e39ac7fd05d9b87613c801eb86cf3dfa33c4097a7fad97` |
| grafana-community/loki | 20.2.0 / Loki 3.7.9 | `4886775896b04ff696a7620921a12de761c57b2e3182acc37639088f348e4f6b` |

`charts.lock.json` records exact upstream release URLs and published archive
digests. All eight referenced runtime images, including the operator-generated
config reloader, are separately pinned to OCI manifest/index digests resolved
from their official registries. Resolution hashes the exact returned manifest
bytes and verifies the registry digest header when supplied. It downloads no
layers and executes no images. Cache corruption or a rendered image outside the
lock fails verification. Digest identity does not replace vulnerability, signature,
architecture, licensing or actual runtime qualification.

Use Python 3 with PyYAML (checked with 6.0.3) and Helm **3.21.0**. From the repo
root, these are the complete local commands:

```sh
# Default is inert: no cache needed, no network, zero rendered objects.
python3 deploy/observability/render.py

# Explicit network step: only the two locked public chart archives are fetched.
python3 deploy/observability/render.py --fetch

# Offline: no repository update, dependency download, cluster lookup or install.
python3 deploy/observability/render.py --qualify-offline
python3 -m py_compile deploy/observability/render.py
```

The cache defaults to `.runtime-test/observability-cache`; it can be copied from a
connected machine and selected with `--cache /absolute/cache`. Every render
rechecks both archives. Output defaults to
`.runtime-test/observability-render/{candidate.yaml,report.json}`; `--output`
selects another local directory. The helper isolates Helm homes/plugins and uses
`KUBECONFIG=/dev/null` with client-only `helm template`. It rejects changes to the
disabled activation flag. There is deliberately no activation path here.

Qualification renders 87 objects, keeps upstream defaults for dashboards/rules,
checks private ClusterIP services, no route/host collector/host mount/privilege,
mandatory Secret references, resource/storage/retention limits and exact immutable
image identities. Six negative controls reject a public service, missing network
isolation, anonymous Grafana, an unbounded container, a floating image and a
corrupted archive. [qualification.json](qualification.json) records the measured
offline result and input hashes. This checks Helm values schemas and rendered
content, not admission against actual installed CRDs or operator-generated Pods.

## Candidate budgets and closed boundaries

| Component | Container memory limit | Persistent request | Retention |
| --- | --- | --- | --- |
| Prometheus | 2560 MiB plus 64 MiB reloader | 30 GiB | 15 days and 20 GB TSDB cap |
| Alertmanager | 256 MiB plus 64 MiB reloader | 1 GiB | 120 hours |
| Grafana and two dashboard/datasource sidecars | 768 MiB total | 2 GiB | state retained on PVC |
| Prometheus operator and namespace-scoped kube-state-metrics | 384 MiB total | none | no durable store |
| Loki monolithic, one replica / replication factor one | 2 GiB | 20 GiB | 14 days in S3, explicit compactor |

The total is 6 GiB container memory limits, 3.35 CPU limits and 53 GiB persistent
requests, leaving a *proposed*, unmeasured OS/K3s/disk headroom budget on the
architecture's 4 CPU / 8 GiB / 100 GiB candidate. Each rendered container has CPU,
memory and ephemeral-storage requests/limits. Operator-generated reloaders have
explicit CPU/memory flags; their final Pods and aggregate ephemeral-storage
accounting still require runtime inspection. A PVC request and Kubernetes
ephemeral-storage eviction are not proven physical filesystem quotas. The
`webdock-obs-storage-unqualified` class is deliberately unbound; do not replace it
with a default local-path class and claim hard disk isolation. Prometheus's byte
retention is not a cap on WAL/head/compaction space; 30 GiB must be qualified under
load with free-space alarms. Loki's local volume retains WAL, index and compactor
state; S3 does not remove that disk requirement. PVCs are retained on Loki
scale-down/deletion.

Metrics discovery/watch is restricted to the OBS namespace, kube-state-metrics
uses a namespaced Role and selected namespaced collectors, and host exporters and
other control-plane scraping are off. Remote-write receiving and the Prometheus
admin API are off. Sample/target/label limits and query concurrency are bounded.
The upstream Alertmanager config has a null receiver; actual paging is not enabled.

Loki uses TSDB schema v13, TLS S3 settings, a 2 MiB/s tenant ingestion budget with
4 MiB burst, 1000 streams, bounded per-stream rates/line/label sizes and bounded
queries. Its endpoint, region, bucket names and keys are **unset environment
references** supplied only by missing private Secret `obs-loki-s3-unconfigured`.
Grafana similarly references missing private Secret
`obs-grafana-admin-unconfigured`; no password is generated or stored here.
Grafana anonymous access and signup are off, and its local login form is disabled
pending the Webdock sign-in/MFA integration. Neither missing Secrets nor a hidden
login form is an authentication design.

There is no ingress, default Loki gateway, canary, external log collector, embedded
MinIO or large cache deployment. Chart 20.2.0 has removed the old `minio` values key;
older example values fail its schema. The namespace-wide deny-all ingress **and**
egress policy has no exceptions. Thus even platform ingestion, intra-stack
scraping and S3 are closed in this candidate. NetworkPolicy enforcement and
host/control-plane bypass behavior remain unqualified. It must not be treated as
an authenticated tenant gateway. `auth_enabled: true` and `X-Scope-OrgID` select a
tenant; they do not authenticate the caller. Customer collection/query remains
closed. The durable customer build-log API/store remains authoritative.

## Gates before any activation

1. Review the separate OBS provisioning diff, measured ingestion/cardinality/query
   workload, resource/disk headroom and actual monthly node/storage/transfer cost
   with the owner. No purchase or Pulumi state is created by this foundation.
2. Qualify pinned images and actual K3s/CRD/operator compatibility on a disposable
   target. Test CPU/memory pressure, physical disk bounds, restart/WAL replay,
   retention/compaction deletion, full disk, source/receiver/S3 outages and restore.
   Establish RPO/RTO, lost-log behavior and an independent dead-man alert.
3. Bind separate private OBS buckets/IAM/Secret delivery/TLS endpoints, test
   read/write/delete denial and lifecycle/versioning/Object Lock behavior, and
   qualify actual storage-class retention/quotas. Wasabi remains only a candidate;
   check the actual account's minimum-duration/minimum-billing and compaction
   costs before selecting short-retention storage. Never extend sensitive-log
   retention merely to match billing terms.
4. Integrate operator-only Webdock sign-in and MFA-compatible Grafana access,
   session revocation, private TLS routing and least-privilege access. Qualify the
   network policy/CNI and narrowly reviewed DNS/S3/API/source flows before opening
   any exception; do not remove deny-all just to make a chart become Ready.
5. Before Loki can receive or query data, qualify an authenticated private tenant
   gateway: strip caller tenant headers, derive current allowed scope server-side,
   enforce quotas and test forged headers, two-tenant denial, revoked identities
   and compromised collectors. Platform-only first; no customer source or shared
   privileged Grafana datasource as an authorization boundary.
6. Optional Alloy is a later trusted collector plan, not a deployed dependency:
   inventory platform emitters first, collect outbound through authenticated TLS,
   use bounded labels and structured metadata, finite local buffer/disk/retry
   budgets with backpressure, and failure tests proving OBS loss cannot block RUN
   or BUILD or fill their disks. Review any host mounts/API read authority
   separately. Customer Pod labels/output cannot establish tenant ownership.
7. Choose one alert owner/routing path, resolve noisy or absent-metric upstream
   rules, configure real notification credentials through private Secrets, test
   delivery and dead-man alarms, and prove Grafana/Alertmanager backup/restore.
   A single OBS node has maintenance downtime; S3 does not make Loki HA.

## Primary upstream references

- [Prometheus community chart source](https://github.com/prometheus-community/helm-charts/tree/main/charts/kube-prometheus-stack)
  and [official chart index](https://prometheus-community.github.io/helm-charts/index.yaml).
- [Grafana community Loki source](https://github.com/grafana-community/helm-charts/tree/main/charts/loki)
  and [official chart index](https://grafana-community.github.io/helm-charts/index.yaml).
- [Loki monolithic mode and chart ownership](https://grafana.com/docs/loki/latest/setup/install/helm/install-monolithic/),
  [authentication boundary](https://grafana.com/docs/loki/latest/operations/authentication/),
  and [retention](https://grafana.com/docs/loki/latest/operations/storage/retention/).
- [Prometheus local storage limitations](https://prometheus.io/docs/prometheus/latest/storage/)
  and [Alloy Kubernetes collection](https://grafana.com/docs/alloy/latest/collect/logs-in-kubernetes/).
