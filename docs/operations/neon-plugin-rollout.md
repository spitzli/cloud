# Neon database plugin rollout

The database-first runtime is maintained in `spitzli/cloud-plugin-neon`. All API
keys, attachments and idempotency receipts belong to the existing Core database.
The runtime has no database, persistent workspace, provider key or direct Neon
network access. Public catalog publication and a working customer installation
are separate acceptance steps.

## Compatibility and migration

### Owned acceptance, 2026-11-04

Neon `0.1.2` is signed, installed and promoted through the normal managed update
path as runtime generation 4. Its complete selected inventory returned project,
branch, database, role and endpoint through the public Console gateway in an owned
acceptance run taking **18.94 seconds**. This is one measured run, not a latency
SLA. Gateway, SDK and invocation deadlines remain unchanged. At most five fixed
GET callbacks overlap, with independent Core authorization/revalidation and the
same safe-field projections. An independent source review found no issues.

The `0.1.1` baseline passed attach/replay, settings, branches, protected
pooled/direct URI retrieval and real TLS-verified read-only SQL. A one-attachment
limit refused a second attachment, and project binding refused access from the
other owned project. Local detach worked after removing creation permission and
disconnecting the provider connection; it released the allocation and retained
the provider database. Version `0.1.2` changes only inventory concurrency; these
write and credential checks are recorded with their original tested version.

During a second `0.1.2` inventory request, the owner disconnected the provider
connection. The in-flight request then returned a sanitized HTTP 503 with no
inventory. The temporary scoped key was revoked with HTTP 401 confirmed, its
plaintext removed, and the workspace left disconnected with zero attachments.
No provider resources were created or deleted. No website-authority gate or
namespace admission rule was relaxed. The old signed releases and runtime
material remain available for controlled rollback.

These checks used the real operator/owner, not an ordinary customer identity.
The full browser/customer acceptance remains separate. See the
[dated acceptance record](../verification/2026-11-04-neon-owned-acceptance.json).

### Verified checkpoint, 2026-11-03

Core HTTP, agent-control, plugin-control and plugin-controller now run source
`6014971aa9b38fcc18fbd2f7ae186c470c9a8a89`, merged through PR #62, at
`webdock.local/core/backend@sha256:a0845a12911f7a24c3630a3b9de67f5d6c984a9864baf33e935cc4493bea4446`.
The actual OCI executables passed initial/repeated migrations and Core readiness
against isolated PostgreSQL 18 before deployment. All four deployments are ready.
The production schema remains at 0075; this rollout required no migration.
All eight protected identity/authorization table hashes and the existing runtime
Secret and issuer material were verified unchanged across the rollout.

Only the reviewed callback hostname and matching host alias changed in a new
immutable common configuration Secret. The retained prior image ending
`179500fd022fca0045396bbac35be3306277cb38fc3ca7f0d8fb7a98466c3133`
remains a schema-0075-compatible rollback. Initial Neon callback recovery has
been requested through the ordinary MFA operator API. Runtime promotion and
customer/provider acceptance remain pending; Core readiness does not prove them.
Preparing the new namespace foundation currently requires Run-server SSH access,
whose existing 1Password signing agent was unavailable at this checkpoint.

### Earlier pre-migration baseline

Before the 0075 rollout, production HTTP Core ran the `3342b745` compatibility checkpoint
(on the `151472be` production source), with native SQL through **0072**. Do not
replace that image with a branch head merely because source checks pass. Qualify
all intended changes and retain the existing environment and secret custody.
The candidate needs the accepted **0073, 0074 and 0075** migration bytes. Apply them
with the existing explicit offline migrator against the verified database target;
never migrate with an ordinary runtime role or put an owner URL into serving Core.

`native_schema_ready` compares the complete migration journal with the executable.
After applying 0075, the old 0072 image cannot be assumed to become ready again.
Before migrating production, prepare and qualify a rollback compatibility image:
retain the old serving application behavior and keys, but bundle the exact new
accepted migration files so its readiness check understands the retained journal.
Do not undo schema/data or delete the attachment tables to make an old image ready.
Keep the prior and rollback image digests, exact configuration and custody backup.

0075 is additive: protected attachment and nonsecret receipt tables, an atomic
local attachment/index/reservation commit and narrow reclamation of a revoked or
expired **uncommitted local Neon attach**. It neither creates nor deletes provider
resources and does not make uncertain external plugin creates generally retryable.
Accept its frozen checksum only after restricted-role integration, upgrade/repeat
and source review checks. A frozen source checksum is not a production migration
receipt.

## Existing infrastructure and private control

Use the existing Core and enrolled Run Kubernetes agent. No extra VPS, VM, plugin
metadata service, CNPG cluster or PVC for Neon metadata is required. Inspect the
current enrollment, native runtime capability, private network addresses, Traefik
entrypoint and enforced network policies before producing a target profile.

The inspected serving Core has an empty file-based runtime map and a Unix plugin
configuration. Merely setting `SPITZLI_NATIVE_PLUGIN_CONFIG` does not start the
private callback listener or installer. Separately supervise:

```sh
spitzli-plugin-control --config <private-plugin-config>
spitzli-plugin-controller --config <private-plugin-config> \
  --installation-config <private-worker-config>
```

For the managed installer, use the reviewed pinned mTLS listener and private
catalog. Reuse the existing invocation signing key and installation encryption
secret. Inspect retained plugin PKI before preparing missing material; do not run
fresh bootstrap over an existing installation. Only the installer gets a kernel
CA signing key. Core HTTP and callback need their retained client/server identity,
public CA and narrow private configuration, not that CA signing key.

HTTP and callback must use the same Neon connection encryption key selection:
`SPITZLI_HOSTING_ENCRYPTION_KEY`, then `BETTER_AUTH_SECRET`, then
`SPITZLI_AUTH_SECRET`. Preserve the currently selected key. Private browser/OAuth
authority uses the retained protocol issuer, independently of the canonical public
login hostname. Do not rotate MFA, passkey identities, user passwords or keys as
part of this rollout.

Worker configuration for this stateless plugin:

```json
{"storageTargets":[]}
```

Empty targets do not require `pg_control_system` privileges on Neon. Stateful
installation preparation still obtains a fresh Core cluster identity and proves
its metadata database is a separate cluster before provisioning it.

## Signed catalog and managed installation

Use the correct `registry.spitzli.cloud` instance and retain its configured signing
identity (`registry.webdock.dev`, pinned Ed25519 key). Load a release only after
verifying identical manifest/descriptor bytes, the signed receipt, immutable image
digest, actual registry visibility and exact source presentation. Never admit the
authoring manifest's zero image digest. The runtime image must be pullable by the
existing agent profile, which does not add an imagePullSecret automatically.

The private catalog entry uses only:

```json
{
  "recipe": {
    "storage": false,
    "sealingKey": false,
    "routeDomains": [],
    "privateConfiguration": {}
  },
  "supportedFeatures": ["connect-databases"]
}
```

Add this to the existing catalog; preserve other keys, releases and targets. The
actual target needs the verified enrolled cluster, private Core callback origin,
matching pinned leaf and kernel CA, runtime identity issuer, bounded resources
and narrowly allowed callback egress. Do not substitute public IPs for unverified
private routing. Runtime ingress goes through the existing approved Traefik path
or the profile's exact control-plane namespace/pod labels.

With a current authorized operator browser session, obtain the server's review:

```text
GET /api/console/plugins/catalog
POST /api/console/plugins/install
```

Supply `catalogID: neon`, the exact release version, returned `reviewSha256`,
reviewed `targetID`, empty configuration, a stable idempotency key and exactly
`plugin.http.read` plus `plugin.http.request`. Follow the returned operation to
success. Kubernetes readiness alone is insufficient: Core must verify pinned
Describe/Health identity and the exact finalized manifest before promotion.
Use normal retry/disable/update/rollback APIs for failures; do not SQL-repair
installation rows or replace immutable runtime Secrets manually.

The enrolled agent-control process must load the same reviewed native catalog
and retained installation encryption key as HTTP/controller. It also needs the
existing first-party browser client ID/secret/cookie secret and retained OAuth
issuer to revalidate installation authority. Reference the existing restricted
Secret keys; do not create another client, rotate credentials or bypass a blocked
`authority_changed` operation. Retry through the normal operator API after the
configuration has been corrected.

Run application admission remains enabled for plugin Pods. Provision and verify
the UID-fenced namespace foundation with
`tools/plugin-runtime/harden_namespace.py` as described in that directory's
README. The fixed agent profile must include startup and liveness probes as well
as readiness. If admission refuses the initial Pod, retain the failed operation
and inspect its actual reason; a namespace label or a ready Deployment alone
cannot substitute for quota, isolation, pinned Describe and Health verification.

## Customer acceptance

Enable the reviewed `connect-databases` feature and a finite `neon.database`
attachment limit through existing operator plan/customer controls. A new plugin
key is not automatically inherited from a retired `postgres.database` limit;
review the customer's effective limits before granting access.

Connect a customer-scoped Neon API key in Workspace Connections, then verify the
actual browser-authority flow: discover → attach → safe settings/branches →
explicit write-authorized URI reveal → local detach. Initial attachments require
actual provider metadata in Frankfurt (`aws-eu-central-1`). Do not create paid
provider resources, move existing projects or contact private customer projects
merely to manufacture a smoke test. Use a specifically owned authorized database.
Use an authorized application/database project for that test. Do not place the
platform's Core Neon project key in a customer workspace to manufacture provider
acceptance; that would grant customer administrators unrelated platform access.

Prove tenant separation, ordinary member/read-only behavior, key/session/grant
revocation during requests, stable retries, quota allocation and recovery after
runtime restart. Reveal responses must be `no-store`, absent from persisted
receipts/shared caches/logs, and absent from screenshot capture. API-token/MCP/job
invocation is deliberately unsupported for this first browser release. Detach
must still work locally with the creation feature disabled or provider unavailable,
and must leave the Neon database untouched.

The synthetic Console fixture, browser tests and gallery verify UI contracts and
responsive layout. They do not prove a production provider connection. Record
source/check/image/migration/installation/customer evidence separately before
reporting the integration live.

## Recover an unpromoted callback hostname

The explicit operator endpoint `POST /api/console/plugins/neon/recover-callback`
repairs one narrowly defined initial-installation failure: a private IP callback
URL was prepared with a different retained TLS DNS name. Review a catalog target
whose callback URL uses that DNS name and whose single host alias resolves it to
the original private IPv4 address. The original callback port, egress, certificate
pin, issuer, resource bounds and other target fields must match exactly.

The request uses the reviewed catalog `version` and `reviewSha256`, the current
installation `expectedRevision`, unchanged `approvedCapabilities` and a fresh,
persisted `idempotencyKey`, as for a release update. It accepts only the same
signed release of an explicitly stateless candidate that has never been promoted.
Stateful, promoted, incompatible, stale and differently placed installations are
refused. This endpoint is an infrastructure recovery operation, not a customer
configuration option.

Recovery fences the prior operation and retains its immutable material and UID
checkpoint. The normal agent must suspend the prior generation and prove that no
Pods remain before it can apply the new generation. Prepare the new namespace's
quota and isolation foundation, then follow the operation through pinned
Describe/Health verification. A repeated request with the same key replays the
same operation. Existing availability and update operations retain their guards.

Core validates newly generated TLS transport configuration using the same SDK
validator as the runtime before dispatch. Certificate checks and the runtime SDK
remain unchanged. No SQL installation repair, Secret replacement or release-tag
overwrite is part of this procedure.
