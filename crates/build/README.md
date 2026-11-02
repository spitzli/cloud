# Rust Git builds and OCI publication

`webdock-build` owns retained Git source/build/release state and the isolated OCI worker machinery. Core HTTP adapters remain outside this crate. The provider adapter is unavailable by default: no network provider, artifact import, or build is reported as successful without a real configured implementation.

The service uses restricted `webdock_auth_runtime` and `webdock_worker_runtime` pools, signed session/fixed `git-worker` RLS scopes, an injected dedicated-pool `IdAllocator`, and `HostingService`. Native migration 0012 points to `migrations/0001_git_authority.sql`; it is additive and runs inside the native migration journal transaction. Existing receipts are retained, with new exact completion hashes for native retry handling. Legacy artifacts without a verified OCI manifest digest cannot enter native publication.

## API

- `BuildService::new(session_db, worker_db, rls_secret, ids, hosting, registry_host)`; `with_repository_provider(Arc<dyn RepositoryProvider>)` supplies installation/repository verification, immutable commit resolution, and normalized source archives.
- `execute(&Principal, Value)` handles customer connection/repository reads and disconnect, source list/get/configure, build request/list/get/cancel, release list/approve/rollback. Retained `projectID`, `targetID`, `sourceSHA` and pagination names are preserved. Build requests select an app target; multiple sources require an explicit target. Source environment values remain encrypted; only names are returned.
- `enroll_worker`, `revoke_worker` require a current mapped operator. `authenticate_worker` returns opaque `WorkerIdentity`; every worker action checks its current persisted generation, enabled state and credential hash.
- `claim_build`, `guard_build`, `source_archive`, `complete_build` bind source/connection/environment revisions and the worker generation. Build leases last 27 minutes. Completion atomically writes artifact, release, outbox and build state. Exact retries return the prior receipt; changed retries fail.
- `claim_release`, `guard_release`, `prepare_publication`, `publish_image`, `observe_release` bind live approval and worker leases. Publication expires into reconciliation, never a blind requeue. `publish_local_artifact` combines actual private-store verification, guarded OCI registry upload, and the trusted atomic Hosting SPI.
- `Archive`, `ArtifactStore`, `VmImage`, `GuestRequest` are worker-local components. `webdock-build-guest` runs only inside the disposable VM. Production OCI publishing uses TLS; the `local-registry-test` feature exposes only an explicit loopback HTTP constructor.

Automatic `git-policy` approval is a durable source policy, not a fabricated browser session. Native helpers check current source revision, approved policy subject, active verified/unbanned account, locally authoritative identity and organization, and current owner/admin membership (or explicit MFA-enabled mapped operator). Manual approval additionally requires its real stored unexpired session. Agent continuation binds the persisted release's exact operation, project, app and image. Hosting app/revision/reservation/operation staging and release linkage commit in one worker transaction.

## Isolation and evidence

Customer Dockerfiles run in QEMU/KVM with a digest-pinned raw root image and optional separately pinned boot files. Each invocation uses a fresh snapshot, no host filesystem shares, no network device, sanitized guest request, bounded CPU/memory/disk/log/time, whole-process-group cleanup and live lease checks. Archives reject traversal, links, sparse entries, device files, duplicate names and oversized payloads. Executable bits survive normalization; privileged file bits do not. OCI descriptors are recursively size/hash verified, and publisher writes original manifest bytes rather than reserializing them.

The explicit `dockerfile-frontend` recipe additionally retains selected regular
files from the final image rootfs. It requires bounded `.webdock/frontend.json`
configuration, an explicitly capable pinned VM and format-2 readers. It preserves
the original OCI archive and source evidence within the common artifact budget.
See [frontend retention operations](../../docs/operations/build-frontend-retention.md)
for compatibility, private library reuse and owned qualification boundaries.

The local fixture was prepared from the [official Alpine 3.22.6 release](https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/x86_64/) (SHA-256 pinned in the preparation script) and signed Alpine packages. `tools/prepare-local-vm.sh --allow-disposable-local-vm` requires a fresh destination and never reads production images. It records package versions and hashes. Runtime builds cannot use the preparation network.

Tests require explicit owned local database URLs and never reset shared schema or fixtures. `tests/support/hosting_flow.rs` reuses the Hosting test fixture, creates a real local Git commit, builds its claimed archive in KVM, publishes to a temporary pinned loopback registry, and stages/completes the retained hosting operation. This does not exercise a real Kubernetes deployment; that executor has separate acceptance tests.

```sh
set -a
source .runtime-test/identity-restricted.env
set +a
cargo test -p webdock-build --no-default-features --test artifact --test contracts
WEBDOCK_BUILD_VM_TEST=1 cargo test -p webdock-build --no-default-features --release --test vm -- --include-ignored
WEBDOCK_BUILD_REGISTRY_TEST=1 cargo test -p webdock-build --all-features --test publisher -- --include-ignored
WEBDOCK_BUILD_VM_TEST=1 WEBDOCK_BUILD_REGISTRY_TEST=1 cargo test -p webdock-hosting --test postgres git_build_publication -- --include-ignored
```

## Remaining parity

Provider OAuth/installation onboarding, verified webhook receipt drains, external CI artifact import/provenance and provider-specific recipe execution require the external provider adapter. The retained outbox is written atomically; remote check delivery remains unavailable until that adapter is connected. Release reconciliation of ambiguous external writes and artifact retention/orphan cleanup remain to be ported. The native worker HTTP transport, private daemon configuration, durable completion outbox and normalized provider source archives are implemented. Publication observation still needs a durable local phase record across worker restart. Native Docker private-registry credential setup must advertise real registry capability before Git publication is admitted. No production cutover is claimed.


## Independent repository runtime

Native0013 (`migrations/0002_repository_authority.sql`) adds separately purpose-bound repository leases and `git_connection.repository_plugin_id`. Binding an existing connection is an explicit operator migration; changing that binding automatically advances the connection generation. There is no implicit GitHub binding for old connections.

Construct `RepositoryAdmission::new(worker_db, rls_secret, Arc<HostService>, issuer_signing_key, Arc<FileRepositoryBindings>)`, serve `Arc<RepositoryAdmission>::authority_service()` over a private same-UID socket or verified client-certificate TLS, and inject `PluginRepositoryProvider::new(admission)` into BuildService. The authority service rejects plain unauthenticated network requests. Publisher keys, signed repository descriptors and transport identities are operator-pinned in the private bindings file. HostService separately checks its existing installation/runtime credential, manifest, grant, capability, configuration and project bindings on every callback.

Repository leases last at most 60 seconds and are distinct from ordinary session Invoke tokens. Interactive resolution borrows the actual Principal and caller transaction. Archive and release work binds persisted worker enrollment, credential generation, source revision and real durable build/release authority. Configuration changes, provider rebinding, grant revocation, account bans and enrollment rotation invalidate active requests. Archives use authenticated ordered 32 KiB frames with a final verified size and digest; the retained 128 MiB compressed and expanded source ceilings are preserved. Source payloads never enter an oversized JSON Invoke envelope.

`plugins/github-repository` is independently built and installed. Core does not link it or need its source in the Core image context. The initial provider supports immutable commit/branch resolution and normalized source archive download. OAuth onboarding, webhook drains, Actions artifact provenance and check outbox delivery are still unavailable. Native repository `Check` is reserved but deliberately has no successful domain-authority implementation yet.

`tests/support/repository_flow.rs` reuses the owned Hosting database fixture and runs a real repository runtime plus Core callback server. Its live-revocation test covers configuration changes during an archive request, browser expiry under current durable build authority, runtime credential rotation, revoked grants, banned accounts, provider rebinding and unsigned worker RLS denial.
