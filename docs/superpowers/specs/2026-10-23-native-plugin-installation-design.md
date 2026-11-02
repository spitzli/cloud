# Native initial plugin installation

Status: implementation authorized by the owner's “go” after the concrete Console-to-PostgreSQL milestone. Technical calls below are the implementer's choices.

## Intent

A fresh native installation can install a reviewed, signed PostgreSQL product plugin from Console, provision its private metadata database/configuration, run the actual OCI artifact through its enrolled Kubernetes agent, enable a supported self-service feature, create a customer resource and execute SQL. No development seed, manual installation/enrollment SQL, dummy customer or dummy project establishes success.

## Boundaries and choices

- Rust Core remains the only active platform API. React/Vite Console and the existing bundled PostgreSQL resource UI remain in place. Product business rules stay in the plugin.
- Initial installation is this milestone. General jobs, source builds, upgrades, rollback and purge remain subsequent work; availability disable retains its existing admission/draining meaning.
- Use an operator-owned catalog of signed release bundles, pinned registry identities and Ed25519 keys. A public remote registry is not required. Browser input never supplies trusted keys, manifests, images, private configuration, endpoints or Kubernetes objects.
- Keep plugin metadata storage on an explicitly configured PostgreSQL storage service, with one normal restricted login and database per physical platform/plugin installation. The provisioning administrator is private to the separately supervised install worker, never a plugin or ordinary request-serving database role. Before provisioning DDL, compare actual PostgreSQL `system_identifier` values from `pg_catalog.pg_control_system()` for platform and metadata storage, independently of listening interface or database name. Reobserve platform identity before each provision and fail closed when either identity is unavailable; do not grant monitoring roles or replace this check with configured identifiers. Use verified PostgreSQL TLS for remote storage; only explicit disposable loopback fixtures may use plaintext.
- Runtime deployment is installation-owned on the existing enrolled outbound agent channel. Customer/project workload tables and customer application RuntimeSpec must not stand in for this ownership.
- Reuse existing mTLS runtime/client libraries, pinned Core/runtime leaves, exact signed manifests, protected outboxes and Kubernetes ownership/convergence patterns. Keep local Unix runtime configuration supported.
- The initial managed profile uses the bootstrap-issued single private kernel CA and one Core leaf for server/client identity. Mixed kernel issuers or distinct Core client/server leaves are rejected before provisioning; metadata storage has its own independently configured CA. This is a profile boundary, not an insecure fallback. Future profiles may support separate kernel chains explicitly.
- Private Secret projections are staged by the reviewed runtime image into a runtime-owned memory volume as regular mode0600 files. Preserve private-file reader ownership/link/symlink checks. The actual runtime mounts only the private staged copy.
- A target is operator configuration: enrolled verified EU cluster, runtime DNS/Traefik endpoint, private Core callback, tightly bounded Core/storage egress, optional reviewed host aliases, resources and storage target identity. Browser selects its ID, not its implementation details.
- The first PostgreSQL managed profile exposes dedicated instances. Shared creation stays unavailable until shared servers are actually configured; a generic installation feature ceiling enforces this in both UI and admission.

## Release trust

Native verification matches the existing integrity contract: compact EdDSA JWS; protected and payload key IDs; exact registry, slug, version and manifest SHA256; exact descriptor/manifest byte equality; matching universal asset name/hash/UTF8 size. Reject duplicate JSON keys, unknown critical JOSE behavior, malformed/wrong signatures and identities, oversized data and placeholder all-zero image digests. Manifest protocol/sdk/data versions are2; runtime wire Health protocol is1.

Catalog review exposes only public release metadata, supported features, capability/workload declarations, compatible targets and a review digest binding manifest, recipe and target policy. Private recipe fields and administrator credentials never become public configuration or operation payloads.

## Admission and durability

Initial request consumes an exact catalog/release/review digest, approved capability set, target ID, validated public configuration and idempotency key. Current write-capable platform operator authority is required; customer, read-only and acting scopes are denied. Exact retries return the same operation; changed intent under the same key conflicts.

Reuse plugin_installation/release/configuration/runtime/operation records. Persist signature provenance, approved target/recipe fingerprints and encrypted execution material in a narrow new installation execution record. Persist generated storage passwords, sealing keys and runtime identity once before external work. Never regenerate them during recovery.

Phases: requested → preparing_storage → preparing_configuration → deploying → verifying_runtime → complete. Public snapshots include operation kind and safe condition codes. Claims are fenced by operation generation, token, expiry and current retained operator authority. Short claim/renew/guard transactions surround external work; no database transaction spans a rollout wait.

The install worker provisions storage/configuration and exposes guarded dispatch. The agent claims deployment on its authenticated connection, revalidates before/after mutations and during waits, persists nonsecret UID checkpoints and exact protected receipts, and replays receipts after lost acknowledgments. Secrets travel only in ephemeral material and the intended Kubernetes Secret; they do not enter general command journals, checkpoints, events, audits or browser responses.

Unknown outcomes reconcile the same owned resources. Existing resource ownership/spec mismatches fail closed. Failure and authority revocation retain database/configuration/resources without reporting success. Explicit retry under fresh operator authority reuses the approved release, installation/runtime identity and material, with a new operation fence. Purge is unavailable.

## Discovery and promotion

RemoteAuthority already permits Describe/Health to its configured Core peer without requiring active invocation authority. Reuse that probe-only path; do not relax runtime invocation/workload authorization for candidates.

Managed bindings and runtime connections come from one consistent persisted installation execution/runtime record, with existing file-enrolled runtimes retained as a fallback and collisions refused. Candidate connection lookup is private to the installer; ordinary invocation still requires enabled/ready state.

After agent-observed deployment readiness, Core authenticates the exact candidate certificate and compares Describe bytes and Health ID/version/generation/wire protocol. It revalidates catalog/spec/configuration fingerprints, claim and operator authority before atomically publishing the current runtime, enabled availability and successful operation. A TCP port, queued command or Health response alone is insufficient.

## Fresh setup

The supported native empty-database bootstrap initializes core-only plugin state without installing products or creating customers/projects. Existing retained installations are not silently classified as fresh. A supported exact-target offline enrollment command registers a reviewed cluster/agent transport and refuses conflicting replacement; serving Core cannot mint its own transport authority.

An installer target requires a prepared cluster (including the PostgreSQL operator for dedicated resources), private callback trust, a storage service/provisioning identity, and an approved real signed artifact. These are deployment prerequisites, not per-plugin manual database rows. The checked-in zero-digest development manifest is not an installable catalog release.

## Console

Extend Extensions with available releases. Review version/signer/image/capabilities, compatible target and storage requirement; accept once with stable idempotency. Show durable operation phases, refresh/reconnect and explicit safe retry. Success links to existing self-service settings. Default features remain assigned; unsupported features cannot be enabled or invoked.

The supported customer/project/resource flow is unchanged after installation. Installation itself does not require one. No third-party script is downloaded into Console's origin.

## Acceptance

1. Cross-language signature fixtures and all trust rejection/bounds cases.
2. Fresh native schema/operator/plugin bootstrap and supported agent enrollment, with no seeded products or dummy owners.
3. Restricted-role admission, exact replay/conflicts, operator/session revocation, stale leases/generations/specs/receipts and private-material non-disclosure.
4. Real OCI runtime with private staged files, pinned bidirectional mTLS, private migrated storage and denied wrong CA/name/peer.
5. Recovery after each phase and lost replies reuses database, role, credentials, keys, namespace and runtime identity. Replacement retains rows.
6. Real Console install on an empty disposable platform; configure supported self-service, create an intended customer/project through supported APIs, create dedicated PostgreSQL, wait for observed readiness and execute SQL write/read. Restart the runtime and read retained state.
7. Independent review, cargo fmt --all and full bun run quality before signed commit/push. No production resources or real mail.

## Implementation interfaces

Task owners settle minor Rust signatures through the parent before consuming another task's file.

- Agent wire service: `PluginRuntimeControl` with `PollPluginRuntime`, `GuardPluginRuntime`, `CheckpointPluginRuntime`, `CompletePluginRuntime`. Poll uses existing PollRequest; Guard returns existing GuardReply; callbacks return Ack. Command fields mirror the native Database/Mail domain: strict `spec_json`, `lease_json`, `request_sha256`, ephemeral `material_json`, accepted `checkpoint_json`.
- DTO owner is `crates/agent-protocol/src/plugin.rs`. Spec contains platform/plugin/runtime/configuration/cluster identity, manifest/material hashes, digest image, stable namespace/service/runtime host/entry point/port, bounded CPU/memory, reviewed egress and optional host aliases. It contains neither credentials, arbitrary commands nor Kubernetes YAML. Lease contains operation/generation/token/expiry and agent generation. Keep a separate lease/fingerprint from stable owned resource identity.
- Private material is a bounded fixed-filename UTF8 map for `runtime.json`, `manifest.json`, `runtime.crt`, `runtime.key`, `core-ca.crt`, `storage-ca.crt`. Required files are profile validated. Agent stages only these names into a private memory directory and passes the fixed runtime config path. Public manifest bytes need not be encrypted at rest; private components use individually purpose-bound existing SecretCipher envelopes to respect its size limit.
- Backend exposes install admission/catalog/snapshots, preparation claims, prepared spec/material persistence, authenticated agent dispatch/guard/checkpoint/completion, verification claims/promotion, retry and managed binding/endpoint lookup. Agent-control owns transport enrollment checks before calling it. The parent wires the concrete service after DTO/backend signatures are published.
- Product executable keeps optional Unix `listen`/`core` paths and adds mutually exclusive `listenTls`/`coreTls`; TLS files are certificate/privateKey/authority, callback adds exact origin/domain/serverCertificateSha256. Remote metadata storage requires `storageCa` and verified TLS; no silent NoTls fallback.
- Generic metadata provisioner consumes an operator-owned administrator connection/trust and a stable plan: platform/plugin ownership, database/role names, fixed application password, runtime host/port/CA. It returns only the private application connection information. Existing database/role ownership markers must match; retries never rotate credentials or adopt foreign resources. It is constructed only by the installation worker.
- Probe authorization is already separate in RemoteAuthority::authorize_probe. It checks the configured Core peer without granting invocation/workload authority; no broad candidate exception is required.

## Global constraints

Bun only; no new dependency unless necessary for the secure transport/provisioning boundary. Use only disposable local PostgreSQL/Kubernetes fixtures. Never print secrets/full database URLs, read .dev-local/demo-owner.json, change the dev operator password, reset others' work or commit either local handover. Preserve existing Inter/orange/light/dark UI. No production rollout is implied.
