# Native customer Git connection — implementation brief

Date: 2026-11-04. Scope: the next increment of the owner's approved Git-build architecture and
TODO execution queue. This records builder decisions for the first local qualification slice;
provider settings and production activation still require actual evidence. Preview environments,
general-purpose CI and public arbitrary-build admission remain outside this increment.

## Existing behavior and boundaries

- Native `crates/build/src/connections.rs` supports list, repository list and disconnect, using
  current actor/session checks, tenant/project-management authority, revisions and generations.
- `git_connection`, `git_repository` and `git_flow` already exist. `git_flow` binds an opaque
  state hash to subject, session, customer and expiry and has encrypted-token/installation fields.
  Its current fields alone do not establish a replay-safe native exchange/finish protocol.
- `git_connection.installation_id` is globally unique. Preserve that cross-customer restriction
  for the first GitHub increment; do not silently generalize it into multi-provider identifiers.
- `modules/hosting/src/server/git-oauth.ts` and `git-deployments.ts` supply customer-flow behavior
  to port. The legacy GitHub client supplies PKCE, exact redirect/query validation and live
  installation/repository checks; its operator-specific identity model must not become customer
  authority in the native implementation.
- `crates/build/src/repository.rs` already verifies publisher descriptors, runtime bindings,
  peer identity, installed grants and live source leases. Existing Resolve/Archive leases require
  real project/source IDs. A connection flow must not invent those IDs or relax those claims.
- The independent GitHub repository plugin owns provider API calls, App credentials and scoped
  tokens. Its existing source/archive runtime and operator-only catalog entry do not implement
  customer OAuth onboarding. Console's `HostingBuilds.tsx` already reads authorized connections.

## First implementation slice

Implement a local, persistent, session-bound connection-flow contract with a fake pinned runtime
and a freshly migrated restricted PostgreSQL fixture. Keep production onboarding unavailable
until the actual provider runtime/binding and customer flow are qualified. No new server, public
build admission, global registry credentials or direct customer access to internal APIs follows.

### Purpose-separated provider contract

Add typed begin/exchange/options/finish/abort operations in the repository protocol. Each request
binds a real customer, actor/session, random flow ID, plugin identity, runtime generation, expiry,
operation ID and exact request digest. Use a distinct signed purpose and existing pinned transport;
connection authority must never validate as a source/archive lease or the reverse. Add a separate
customer/session admission policy: existing `HostService::repository_policy` is project-grant based
and accepts resolve/archive/check only. Neither that policy nor old required claim fields become
optional for onboarding.

Core owns platform authorization, opaque durable flow state and atomic connection registration.
The plugin owns GitHub endpoints, client secrets, PKCE/code exchange, user identity and live provider
access verification. Prefer opaque provider handles/sealed state stored through the existing Core
metadata boundary; do not provision another metadata database. Provider secrets, codes and user
tokens must not enter generic build requests, archives, browser diagnostics or customer logs.

The browser starts through an authenticated, origin-checked native route. The callback is a narrow
browser navigation endpoint, not an originless general write API. Bind it to the stored flow,
actual current session, immutable configured callback and exact host/path/query validation. API
tokens and read-only/acting customer previews cannot substitute for the actual browser identity.
Recheck normal current tenant authority before and after each provider operation and before commit.
Explicitly require stored browser authority: existing `begin_authority(None)` is a no-op and must
not admit this lane. Keep `StoredAuthority` private to Core and disclose only the required scoped
identity/binding fields to the provider.

### Durable state and retries

Add provider/runtime binding, protected PKCE custody where needed, explicit exchange/finish state
and durable operation receipts to the existing flow schema. Additive migrations preserve existing
rows and distinguish native flows from legacy flows; do not reinterpret old encrypted values.
Use database locks/revisions to consume each stage once and bind each receipt to its request.
Existing flow RLS is tenant-wide. New native flow/receipt mutations must enforce subject and
session ownership plus live revocation checks, not merely add ownership columns. Qualify this
with restricted-role attempts from another member and another session in the same tenant,
while preserving the distinct legacy row behavior.

Commit an explicit in-flight/uncertain operation state before external exchange; rolling back a
transaction after provider success must not restore exchange permission. A lost exchange response
cannot cause a blind second GitHub code exchange. Resolve the existing
operation receipt or close the uncertain flow and require a fresh ordinary authorization. Expiry,
abort, actor/session revocation and runtime-generation changes must remove continued authority.
Cleanup must not delete an already registered connection or provider resources.

### Provider verification and registration

The plugin must verify the OAuth user can access the installation and each selected repository,
that the installation belongs to the configured App, and that it remains enabled. App-level
installation visibility and an `installation_id` callback parameter are not ownership proof.
Bound provider pagination, redirects, response/archive sizes and error disclosure.

Finish rechecks current user/provider access, then atomically consumes the flow and registers the
verified repository selection. Preserve same-installation locking, cross-customer denial, revision
and generation advancement, audit identity and cancellation of stale queued work. Reconnect removes
obsolete repository authority rather than accidentally retaining a previously selected repository.
Replace the selected set atomically; the legacy upsert-only registration is insufficient. Existing
native check-job enqueueing must also respect the source-only capability, rather than implicitly
requiring check-reporting permission.

The current installation inspector requires `checks=write`, while older selection code permits
metadata-only access. Neither policy should be copied blindly. Qualify an explicit source-only lane
with narrow metadata/contents read permissions, and keep write-check reporting behind its separately
advertised capability. Existing signed descriptors/releases remain immutable; negotiate and publish
new capability/version inputs through the normal plugin lifecycle before activation.

Handle signed installation suspension/deletion and repository-removal events so the connection UI
and pending work are invalidated promptly. Current push/workflow-run parsing alone is insufficient.
Live provider authorization remains necessary even after webhook handling is added.

## Ordered delivery and tests

1. Native protocol/schema/authority and fake-runtime tests: one-use flow, concurrent replay,
   lost response, expiry, wrong actor/session/customer/plugin and revoked authority.
2. Plugin OAuth implementation and loopback-only provider fixtures: PKCE, exact callbacks,
   wrong/suspended installation, forbidden/changed repository, pagination, permissions and redaction.
3. Native begin/callback/selection UI, keeping vendor logic in the plugin. Test malicious return
   URLs, duplicate query/cookie fields, missing session, no-store/no-referrer behavior and ordinary
   session reauthentication. Never expose internal kubeconfig, App secret or access token.
4. Fresh restricted PostgreSQL tests: cross-tenant installation conflict, concurrent finish,
   successful reconnect, repository removal, generation invalidation and denied old leases.
5. Qualify the genuine provider binding and one authorized trusted customer's exact-SHA checkout,
   archive, build/log/cancel path and immutable-digest deployment. Branch movement must not change
   the accepted commit. Keep existing deployments serving on build failure or BUILD outage.

Regression homes: control-plane `build_boundaries`, build support `repository_flow`,
`github_webhook`, `kubernetes_admission`, repository-protocol `claims`/`runtime`, and the independent
GitHub plugin's `provider` tests. Retain legacy Git OAuth tests as behavioral references only.

## Actual evidence still required before activation

The registered callback/client settings, currently granted GitHub permissions, reachable pinned
runtime and signed release, ordinary-customer consent, private registry connectivity and hostile
build isolation cannot be established by source inspection. Inventory these rather than guessing
values, changing live credentials, reopening a broad permission grant or accepting a sample build
as isolation proof. Additional provider types and wider installation-sharing semantics need their
own explicit contract changes.

## Builder check

An independent read-only builder review passed this local protocol/schema brief on 2026-11-04.
Its implementation constraints (separate admission, mandatory stored browser authority, native
flow RLS, pre-exchange uncertainty and atomic selection replacement) are incorporated above.
This is design qualification, not implementation, provider activation or public-build readiness.
