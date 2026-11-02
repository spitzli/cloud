# Native initial plugin installation

For immediate background execution after installation, see [Native plugin jobs](native-plugin-jobs.md).

Internal operator guide for the opt-in Rust initial-install path. The first managed PostgreSQL profile exposes dedicated instances. This documents implemented interfaces, not production readiness or a passing end-to-end rollout; root records actual signed-OCI/Console acceptance separately.

Use this flow for a fresh, separately configured platform. Keep existing Tilt services running. Tilt's seeded Echo demo and the historical [plugin state checkpoint](plugin-state-bootstrap.md) are not fresh-install acceptance. Do not reset the development database, create dummy customers/projects, insert installation/enrollment rows manually, or use existing customer data for a fixture.

## Prerequisites and trust boundary

- Complete [native schema and operator setup](../../crates/control-plane/README.md#offline-migrations) against an explicitly reviewed empty database. Startup does not migrate. Empty-database bootstrap initializes core-only plugin state; retained installations are not silently reclassified as fresh.
- Review the EU Kubernetes cluster, location evidence, explicit kubeconfig/context, CNI policy enforcement and digest-pinned artifact availability. Dedicated PostgreSQL additionally needs the reviewed CNPG operator, Traefik PostgreSQL entry point and certificate issuer/DNS prerequisites.
- Provide a separate PostgreSQL metadata service and its verified CA. Each physical platform/plugin installation receives one deterministic private database and normal restricted login. Before provisioning DDL, the worker compares the observed PostgreSQL `system_identifier` from `pg_catalog.pg_control_system()` on both connections. This cluster-wide identifier catches the same server reached through different databases, addresses, ports or IPv4/IPv6 interfaces. The platform identity is read again before each provision, using its existing restricted connection. PostgreSQL 18 exposes this read-only function without a monitoring-role grant; unavailable access fails closed. Its administrator connection belongs only in the separately supervised worker configuration; neither plugins nor ordinary Core database roles receive it.
- Resolve the private Core callback from inside the runtime network. Review exact Core/storage CIDR+TCP-port egress and optional host aliases. A host's loopback address is not the pod's callback endpoint. For disposable fixtures, use an isolated reachable private bridge/listener rather than global firewall changes or changes to Tilt services.
- Keep stable authentication/RLS and installation encryption secrets in protected process environments. `WEBDOCK_NATIVE_PLUGIN_ENCRYPTION_KEY` is preferred; current fallback is `WEBDOCK_AUTH_SECRET`, then `BETTER_AUTH_SECRET`. Core, private listeners and controller must use the same retained installation key.

Managed runtime trust currently supports **one kernel CA and one Core leaf/key used for both client and server identity**. Target `core.certificate`, `core.privateKey`, `core.authority`, runtime issuer CA and Core leaf fingerprint must match the generated kernel identity. Mixed runtime/Core CAs or distinct Core leaves are rejected before runtime leaf generation or storage provisioning. The metadata PostgreSQL CA is independent. Agent RPC enrollment may use its separately reviewed transport CA.

Private JSON, keys, certificates and receipts use absolute regular file paths owned by the reading process UID, mode `0600`, without symlinks/hard links; bootstrap parents/directories are pre-existing canonical mode `0700`. The initial kernel output directory must not exist. Keep administrator connections, private material and mounted runtime configuration out of logs and public configuration.

## Offline kernel initialization and enrollment

Use the dedicated `webdock_offline_migrate` login in `DATABASE_URL_UNPOOLED`, the exact nonsecret `HOST:PORT/DATABASE` target and, remotely, `DATABASE_CA_CERT` containing the verified PEM bundle. The commands verify native history, target and original restricted offline login. They are not runtime startup hooks.

There is a current bootstrap/catalog dependency: the final target needs the Core pin and paths that bootstrap generates. Initialize once with a valid empty private catalog:

```json
{"version":1,"platformID":"platform-example","keys":[],"targets":[],"releases":[]}
```

Kernel input (`kernel-input.json`; placeholders must be replaced with reviewed values):

```json
{
  "platformID": "platform-example",
  "installationCatalog": "/srv/webdock-private/catalog.json",
  "callbackOrigin": "https://core-control.example.test:6140",
  "listenAddress": "<CORE_PRIVATE_BIND_IP>:6140"
}
```

The callback origin has no trailing slash. Its DNS name becomes the Core certificate name.

```sh
cargo run --locked -p webdock-control-plane --bin webdock-bootstrap-plugins -- \
  --target "$EXACT_PLATFORM_TARGET" \
  --config /srv/webdock-private/kernel-input.json \
  --output /srv/webdock-private/kernel
```

The new kernel directory contains `plugin-config.json`, `invocation.key`, empty `host-bindings.json` and `connections.json` (`{"runtimes":{}}`), `ca.crt`, `ca.key`, `core.crt`, `core.key`, and `bootstrap.json`. Preserve all of them. The receipt includes `coreCertificateSha256` and file hashes; copy that fingerprint to target `core.serverCertificateSha256` and inspect only the needed nonsecret fields privately. Do not put additional files in that generated directory.

**Replay limit:** an exact replay verifies the target, destination, input, original catalog bytes and generated files; it never rotates or overwrites keys. After filling the catalog with generated paths/pin and approved releases, replay against that changed catalog refuses. Even a catalog byte change changes its checksum. Keep the initial reviewed catalog/receipt for provenance; do not delete output or restore an empty catalog to force a new bootstrap. This is one-time initialization, not certificate rotation or catalog-update tooling.

Enroll the reviewed cluster/agent using a separate input. Remote enrollment requires an already issued, currently valid client-auth leaf; the enrollment command does not issue that agent certificate.

```json
{
  "platformID": "platform-example",
  "agentID": "cluster-agent",
  "name": "Reviewed EU Kubernetes cluster",
  "country": "de",
  "region": "eu",
  "locationEvidence": "<REVIEWED_LOCATION_EVIDENCE>",
  "transport": {"kind":"mtls","certificate":"/srv/webdock-private/agent.crt"}
}
```

```sh
cargo run --locked -p webdock-control-plane --bin webdock-enroll-agent -- \
  --target "$EXACT_PLATFORM_TARGET" \
  --config /srv/webdock-private/enrollment-input.json \
  --output /srv/webdock-private/enrollment.json
```

For an explicit local Unix transport, `transport` is `{"kind":"unix","uid":<REVIEWED_UID>}`. The private output retains `clusterID`, installation/agent identity, generation, credential and checksum. Copy `clusterID` into the catalog target. Map output `identity.installation`, `identity.agentID` and `identity.generation` to daemon `installation`, `agentId` and `generation`; the daemon authenticates its enrolled TLS/Unix identity, not a pasted bearer credential. Exact replay checks retained records; conflicting replacement/rotation is refused. An uncertain commit retains its private output for verification, not blind recreation. Location/cluster review is an operator responsibility, not automatic discovery by this CLI.

## Curated catalog, target and recipe

Finalize the private catalog **before starting the services**. The following is a shape example, not a runnable release: angle-bracket values are deliberate placeholders and numeric ports/resources are examples, not cluster inventory. `manifestRaw` and `descriptor` must contain the same exact signed UTF8 JSON bytes as strings, not paths or reformatted objects.

```json
{
  "version": 1,
  "platformID": "platform-example",
  "keys": [{
    "registryURL": "https://registry.example.test",
    "kid": "publisher-key",
    "key": {"kty":"OKP","crv":"Ed25519","x":"<BASE64URL_32_BYTE_PUBLIC_KEY>"}
  }],
  "targets": [{
    "id": "managed-eu", "label": "Reviewed EU cluster",
    "clusterID": "<ENROLLED_DECIMAL_CLUSTER_ID>",
    "policy": {
      "runtimeDomain": "plugins.example.test",
      "runtimePort": 8443, "runtimeOriginPort": 55481,
      "runtimeIngressHost": "ingress.example.test",
      "entryPoint": "websecure", "storageID": "metadata",
      "core": {
        "origin": "https://core-control.example.test:6140",
        "domain": "core-control.example.test",
        "certificate": "/srv/webdock-private/kernel/core.crt",
        "privateKey": "/srv/webdock-private/kernel/core.key",
        "authority": "/srv/webdock-private/kernel/ca.crt",
        "serverCertificateSha256": "<CORE_LEAF_SHA256_FROM_BOOTSTRAP_RECEIPT>"
      },
      "runtimeIdentity": {
        "authority": "/srv/webdock-private/kernel/ca.crt",
        "privateKey": "/srv/webdock-private/kernel/ca.key"
      },
      "resources": {"cpuMillicores":100,"memoryBytes":268435456},
      "egress": [
        {"cidr":"<CORE_PRIVATE_IP>/32","port":6140},
        {"cidr":"<METADATA_PRIVATE_IP>/32","port":5432}
      ],
      "hostAliases": [{"ip":"<CORE_PRIVATE_IP>","hostnames":["core-control.example.test"]}]
    }
  }],
  "releases": [{
    "id": "postgres", "version": "<SIGNED_RELEASE_VERSION>",
    "registryURL": "https://registry.example.test", "kid": "publisher-key",
    "jws": "<APPROVED_COMPACT_EDDSA_JWS>",
    "manifestRaw": "<EXACT_SIGNED_MANIFEST_UTF8>",
    "descriptor": "<EXACT_SIGNED_MANIFEST_UTF8>",
    "recipe": {
      "storage": true, "sealingKey": true,
      "routeDomains": ["db.example.test"],
      "privateConfiguration": {
        "sharedServers": [],
        "dedicated": {"domain":"db.example.test","port":5432,"entryPoints":["postgres"],"issuer":"<REVIEWED_CLUSTER_ISSUER>"}
      }
    },
    "supportedFeatures": ["create-dedicated"], "targetIDs": ["managed-eu"]
  }]
}
```

`runtimePort` is the container/Service port. `runtimeOriginPort` is Core's ingress-facing port (default `443`; `55481` above is a fixture example). The route still targets `8443`. `runtimeIngressHost` optionally changes the dial host; TLS server name and `HostSNI` remain the derived installation runtime hostname under `runtimeDomain`. Omit it when normal DNS reaches ingress. Host aliases and CIDRs must reflect the runtime's actual private routes, including any reviewed NAT boundary; IPv6 peers use appropriate prefixes.

The catalog is operator-curated; no remote registry service or install-from-URL endpoint is required. It pins registry URL, key ID and Ed25519 public key. Verification checks compact EdDSA JWS, protected/payload identities, exact manifest/descriptor hash and UTF8 size, and the unique `<plugin>-universal.json` asset. Duplicate JSON keys, unsupported JOSE behavior and zero-digest placeholder images fail closed. Browser requests cannot supply trust keys, manifests, images, recipes or endpoints. Catalogs are bounded private files (8 MiB; at most 32 keys, targets and releases) loaded at process initialization, not hot-reloaded. Use the same reviewed catalog in each Core process. Initial releases are action-only v2 manifests (protocol/SDK/data versions `2`; runtime Health wire version `1`).

Recipe and target policy are private and review-digest-bound. Do not set reserved runtime keys in `privateConfiguration`: `manifest`, `issuerPublicKeyHex`, `generation`, `listen`, `listenTls`, `core`, `coreTls`, `corePeers`, `storageUrl`, `storageCa`, `secretKeyHex`. The worker generates those values. Initial PostgreSQL has no configured shared servers; the installation feature ceiling denies `create-shared`, including operator admission. Self-service remains assigned by default; enabling a supported feature/customer grant is a separate normal Console action.

## Worker and process configuration

Worker-only JSON (`worker.json`):

```json
{
  "storageTargets": [{
    "id": "metadata",
    "adminUrl": "<PRIVATE_METADATA_ADMIN_CONNECTION_STRING>",
    "authority": "/srv/webdock-private/metadata-ca.crt",
    "runtimeHost": "metadata.example.test",
    "runtimePort": 5432
  }]
}
```

Supply a real private PostgreSQL administrator connection through that protected file, never command arguments or public configuration. Its only permitted URL query key is `sslmode`. `runtimeHost`/`runtimePort` describe the endpoint the pod can reach and the metadata certificate must validate that host. Remote metadata connections require verified CA/hostname TLS; there is no silent plaintext fallback. Normal login/database names, password, sealing key and runtime identity are retained before external work; retries refuse foreign ownership and preserve those values. The plugin migrates its own metadata database at startup and receives no provisioning administrator privilege.

Generated `kernel/plugin-config.json` is the Core `PluginConfig`: `hostBindings`, `invocationSigningKey`, `connections`, `installationCatalog`, and `control` with `kind:"mutualTls"`, `address`, `certificate`, `privateKey`, `authority`. The first three are generated absolute paths. Use it for serving Core via `WEBDOCK_NATIVE_PLUGIN_CONFIG`, both private listeners and the controller. Managed bindings/endpoints are resolved from retained installation state using an independent restricted worker pool and matching encryption key; legacy file routing remains a fallback, with collisions refused.

Agent-control listener JSON is a **different** shape (snake-case fields):

```json
{"kind":"mtls","address":"<AGENT_RPC_BIND_IP>:7443","certificate":"/srv/webdock-private/agent-control.crt","key":"/srv/webdock-private/agent-control.key","peer_ca":"/srv/webdock-private/agent-ca.crt"}
```

Its local alternative is `{"kind":"unix","path":"/absolute/private/control.sock","installation":"platform-example","agent_id":"cluster-agent","uid":<REVIEWED_UID>}`. This listener authenticates existing enrollment on every RPC; serving Core never creates its own enrollment.

Extend the agent's reviewed private configuration, retaining its explicit Kubernetes/Helm/chart policy:

```json
{
  "version": 1, "installation": "platform-example", "agentId": "cluster-agent", "generation": 1,
  "stateDirectory": "/srv/webdock-agent-private/state",
  "control": {"kind":"mtls","origin":"https://agent-control.example.test:7443","domain":"agent-control.example.test","ca":"/srv/webdock-agent-private/agent-ca.crt","certificate":"/srv/webdock-agent-private/agent.crt","key":"/srv/webdock-agent-private/agent.key"},
  "runtime": {"kind":"kubernetes","kubeconfig":"/srv/webdock-agent-private/kubeconfig.yaml","context":"<REVIEWED_CONTEXT>","helm":"/usr/local/bin/helm","chart":"/srv/webdock-agent-private/approved-chart","chartSha256":"<REVIEWED_CHART_SHA256>"},
  "pluginRuntimeEnabled": true,
  "workloads": true
}
```

`pluginRuntimeEnabled` enables installation-owned deployment; `workloads` separately enables the installed PostgreSQL plugin's customer workloads. Both require Kubernetes. Core receives neither kubeconfig nor engine credentials. Remote agent RPC uses mandatory mTLS; an explicit local Unix `control` uses `{"kind":"unix","path":"/absolute/private/control.sock"}`. Unsupported optional plugin RPC is negotiated without poisoning the shared agent connection.

Start independently supervised processes with the normal restricted Core environments described in the [Core README](../../crates/control-plane/README.md), plus the retained installation key. Give only the installation controller access to `worker.json` and the runtime issuer private key.

```sh
cargo run --locked -p webdock-control-plane --bin webdock-plugin-control -- --config /srv/webdock-private/kernel/plugin-config.json
cargo run --locked -p webdock-control-plane --bin webdock-agent-control -- --config /srv/webdock-private/agent-control.json
cargo run --locked -p webdock-control-plane --bin webdock-plugin-controller -- --config /srv/webdock-private/kernel/plugin-config.json --installation-config /srv/webdock-private/worker.json
cargo run --locked -p webdock-agent -- --config /srv/webdock-agent-private/agent.json validate
cargo run --locked -p webdock-agent -- --config /srv/webdock-agent-private/agent.json run
```

Without `--installation-config`, the controller handles existing availability work only. `--once` performs one bounded controller iteration, not a complete installation. Keep native Mail opt-ins absent unless independently requested; this flow provisions no mail.

## Install, observe and retry

A current write-capable platform operator session selects a catalog release and exact target review in Console Extensions. Customer, read-only and acting scopes cannot install. The native browser API is:

| Method/path | Purpose |
| --- | --- |
| `GET /api/console/plugins/catalog` | Public release metadata, compatible target reviews and supported features; no private recipe |
| `POST /api/console/plugins/install` | Accept reviewed initial installation |
| `GET /api/console/plugins/installed` | Retained state and latest `operationID` |
| `GET /api/console/plugins/operations/{id}` | Safe operation snapshot |
| `GET /api/console/plugins/operations/{id}/events` | Authorized SSE; resume with `Last-Event-ID` |
| `POST /api/console/plugins/operations/{id}/retry` | Explicit fresh-authority recovery of a failed/blocked initial install |

Installation body uses values returned by catalog review, with one stable idempotency key:

```json
{"catalogID":"postgres","version":"<REVIEWED_VERSION>","reviewSha256":"<REVIEW_SHA256>","targetID":"managed-eu","configuration":{},"approvedCapabilities":["plugin.workloads"],"idempotencyKey":"<STABLE_REQUEST_KEY>"}
```

After an uncertain response, repeat the exact request/key; changed intent conflicts. Normal phases are `requested → preparing_storage → preparing_configuration → deploying → verifying_runtime → complete`. Snapshots include `kind`, status (`pending`, `running`, `succeeded`, `failed`, `blocked`), generation/cursor strings and safe condition codes. Phases can advance between polls; they are progress labels, not readiness proof. The agent reports actual owned UID/convergence evidence; Core then pins the runtime leaf, compares exact Describe/Health identity/bytes and rechecks authority before publishing enabled/ready state. TCP readiness or a queued command alone is not success.

For a failed/blocked initial install, inspect its safe condition and use `{"idempotencyKey":"<NEW_RETRY_KEY>"}` on the current operation's retry route. An uncertain retry response repeats that same key. The new operation fences the old claim and retains the approved release, storage, keys, runtime identity and UID checkpoints. Recovery can resume deployment without reprovisioning. Follow the **new/latest** `operationID`, not the old failed record. Do not repair state with SQL, replace Secrets, broaden policies or allocate a substitute namespace.

After success, use the existing self-service settings and supported APIs to create the intended customer/project/resource and verify SQL write/read and retained runtime restart. Installation itself needs no customer or project. Availability disable/draining retains its existing admission semantics; it does not imply storage deletion.

## Update and rollback

For a native managed installation, open **Versions** at `/extensions/{plugin}/releases`. Explicitly disable the extension and wait for the availability operation to finish draining accepted work before requesting a version change. A current write-capable platform operator reviews the signed release and current installation revision; customer, read-only and acting scopes cannot perform this flow. Legacy/file installations without verified managed provenance are unsupported.

Updates and rollbacks retain the existing target, placement policy and public configuration. The target release must accept that configuration, and its declared capabilities, retained approvals and submitted approvals must be the same set. This flow cannot relocate an installation or grant additional capabilities. The release must remain in the reviewed loaded catalog.

| Method/path | Purpose |
| --- | --- |
| `GET /api/console/plugins/{id}/releases` | Retained release history, current/candidate generations, schema high-water and rollback eligibility |
| `POST /api/console/plugins/{id}/update` | Request an exact reviewed catalog version |
| `POST /api/console/plugins/{id}/rollback` | Select a previously promoted release by its retained generation |

Update request shape; replace placeholders with the current server-provided review and decimal revision:

```json
{"version":"<REVIEWED_VERSION>","reviewSha256":"<REVIEW_SHA256>","expectedRevision":"<CURRENT_DECIMAL_REVISION>","approvedCapabilities":["plugin.workloads"],"idempotencyKey":"<STABLE_REQUEST_KEY>"}
```

Rollback uses `releaseGeneration` instead of `version`; the selected generation and revision are decimal strings:

```json
{"releaseGeneration":"<PROMOTED_DECIMAL_GENERATION>","reviewSha256":"<REVIEW_SHA256>","expectedRevision":"<CURRENT_DECIMAL_REVISION>","approvedCapabilities":["plugin.workloads"],"idempotencyKey":"<STABLE_REQUEST_KEY>"}
```

Use the exact retained capability set rather than treating the example as an approval default. Repeat the exact body and idempotency key after an uncertain response; changed intent conflicts. Follow the returned latest operation through the existing operation page/API. The promoted release and ordinary bindings change only after Core authenticates the exact candidate and atomically promotes it.

Rollback starts a new monotonically increasing runtime generation from the selected retained release; it never reactivates the old generation. In the simple initial release v1 → release v2 → release v1 sequence, rollback uses runtime generation3. Existing initial spec1 resources keep their original identity. Update/rollback spec2 namespaces derive from platform, plugin and generation, with distinct derived hostnames/SNI and newly issued runtime TLS identities. The installation-owned metadata database, role/password and sealing key remain stable; stopped generation namespaces and material are retained.

Before any candidate starts, the agent suspends each potentially running predecessor or failed candidate using its exact retained spec, material and complete six-object UID checkpoint. It conditionally writes replicas0 with the Deployment UID/resourceVersion and a metadata-only suspension fence derived from the exact lease. A fresh lease advances that fence even if replicas were already zero. Completion requires the resulting Deployment generation to be observed and no remaining Pods, including terminating or unready Pods. Missing/replaced custody or an uncertain writer outcome blocks progression and retains resources. Retry the retained operation under current authority; do not substitute namespaces, regenerate master secrets, adopt foreign objects or purge data.

Lifecycle execution requires agent support2 negotiated through both Poll request `maxSpecVersion` and response `supportedSpecVersion`, plus a current authenticated database capability2 observation. Absent/zero wire versions mean1. New agents advertise capability2 only after this connection negotiates2 and reset negotiation on reconnect. An old agent cannot execute lifecycle work, even if the same enrollment generation still has a stale capability2 observation. This also applies when suspending a retained spec1 source.

Schema compatibility trusts the reviewed signed publisher's logical `schemaVersion` and `readableSchema` declarations. The target must read both the existing high-water H and the reserved `max(H, target.schemaVersion)`. Admission reserves that monotonic bound before startup; guards and promotion recheck it, and enabling a retained reader must respect it. Failure, cancellation and retry never lower the bound. An uncertain start or even a crash before startup may conservatively block an older reader. PostgreSQL's physical SQL migration numbers are a separate domain: rollback performs no SQL undo or down migration, and Describe/Health do not prove physical schema integrity.

## Runtime material and limits

The fixed native profile creates only its derived owned Namespace, immutable Secret, Deployment, ClusterIP Service, NetworkPolicy and TLS-passthrough TCP route. Isolation is applied before a pod starts. Metadata storage is external: this runtime creates no metadata PVC and has no purge command.

The Secret contains exactly six UTF8 files:

| File | Content |
| --- | --- |
| `runtime.json` | Generated private executable configuration, including private application storage connection |
| `manifest.json` | Exact signed manifest bytes |
| `runtime.crt`, `runtime.key` | Retained per-runtime identity |
| `core-ca.crt` | Supported kernel/Core trust |
| `storage-ca.crt` | Independent metadata service trust |

The same reviewed digest image stages the group-readable source projection in an init container into memory. The runtime sees only regular single-link files owned by UID `65532`, mode `0600`, under private mode-`0700` `/run/webdock/runtime`; its fixed positional argument is `/run/webdock/runtime/runtime.json`. Private readers are unchanged. Manifest limit is 256 KiB, each other file 64 KiB and the serialized material map 384 KiB.

Protected checkpoints/receipts contain no decrypted file material and replay before new deployment work after restart. Keep the agent state directory and retained Core encryption/identity material. Inspect safe failure categories, owned UIDs, routing and file metadata; do not dump `runtime.json`, Secret data or private PEMs to diagnose startup.

Purge, general jobs/source builds and automated certificate rotation remain future work. The current update/rollback signed-OCI/Console acceptance is a compatible code-only lifecycle exercise and is still in progress; it does not establish physical schema migration or production readiness. Record its fresh fixture results and independent final review separately.

Source contracts: [offline setup](../../crates/control-plane/src/plugin_bootstrap.rs), [catalog](../../crates/plugin-host/src/catalog.rs), [worker](../../crates/control-plane/src/plugin_installation/worker.rs), [agent runtime](../../crates/agent/src/plugin_runtime.rs).
