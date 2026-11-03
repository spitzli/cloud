# Neon database plugin rollout

The database-first runtime is maintained in `spitzli/cloud-plugin-neon`. All API
keys, attachments and idempotency receipts belong to the existing Core database.
The runtime has no database, persistent workspace, provider key or direct Neon
network access. Public catalog publication and a working customer installation
are separate acceptance steps.

## Compatibility and migration

The inspected production HTTP Core runs the `3342b745` compatibility checkpoint
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
configuration. Merely setting `WEBDOCK_NATIVE_PLUGIN_CONFIG` does not start the
private callback listener or installer. Separately supervise:

```sh
webdock-plugin-control --config <private-plugin-config>
webdock-plugin-controller --config <private-plugin-config> \
  --installation-config <private-worker-config>
```

For the managed installer, use the reviewed pinned mTLS listener and private
catalog. Reuse the existing invocation signing key and installation encryption
secret. Inspect retained plugin PKI before preparing missing material; do not run
fresh bootstrap over an existing installation. Only the installer gets a kernel
CA signing key. Core HTTP and callback need their retained client/server identity,
public CA and narrow private configuration, not that CA signing key.

HTTP and callback must use the same Neon connection encryption key selection:
`WEBDOCK_HOSTING_ENCRYPTION_KEY`, then `BETTER_AUTH_SECRET`, then
`WEBDOCK_AUTH_SECRET`. Preserve the currently selected key. Private browser/OAuth
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
