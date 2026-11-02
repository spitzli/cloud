# Native bearer resources

The retained Studio host serves `/api/registry`, `/api/hosting` and `/api/mcp`.
They accept bearer tokens for the retained Studio MCP resource, never browser
cookies. The Console browser adapters remain separate. Do not replace the
registered Studio issuer, audience or protocol URLs when moving the UI.

Set `WEBDOCK_RESOURCE_CLIENT_ID` and `WEBDOCK_RESOURCE_CLIENT_SECRET_FILE` to an
existing confidential client authorized to introspect the MCP resource. The
secret file uses the same private-file checks as other Core credentials. These
settings work with no plugin installation or plugin control listener. Existing
plugin `mcpClient` configuration remains a compatibility fallback when the new
settings are absent; an explicitly configured but invalid client never falls
back. Client registration and resource linking remain explicit offline actions.

Registry requires `webdock:read`, and writes additionally require
`webdock:write`, a live operator session and a mapped Registry operator. Hosting
requires the separate `hosting:read`/`hosting:write` scopes and current resource
permissions. Mixed Hosting/Registry scope tokens are rejected. Read-only tokens
cannot acquire write access by invoking a hidden tool or changing HTTP verbs.

Every native domain request retains an exact access-token reference and its
bootstrap authorization transaction until the domain commits. Domains recheck
expiry, session, client/resource state and token-row identity before commit;
Resource policies and both issuer/introspector resource links are protected by
shared row locks through commit. Locks acquire resource identifiers first, then
resource/client/link IDs in deterministic order; a revocation already committed
prevents the next request. Registry still audits atomically and Hosting still enforces RLS, customer/project
permissions, quotas, installation state, idempotency and operation fences.
Accepted Hosting work retains its existing durable operation authorization;
this adapter does not change agent lease semantics.

## Registry

The native routes retain discovery, list/search/filter/pagination, read,
customer/project create (`201`), update (`200`) and archive/restore. Audit events
are read-only. String IDs and immutable project ownership are unchanged.
Creation is not idempotent: ambiguous writes must be inspected before retrying.

MCP tools: `list_records`, `get_record`, `save_customer`, `save_project`,
`set_archived`. Mutation tools are absent for read-only tokens; directly calling
one returns the retained HTTP `403` scope challenge. Plugin tools keep their
`plugin__…` namespace and existing authorization behavior.

`resource-registry.schema.json` is captured from the retained first-party
contracts by `tools/capture-registry-contracts.mjs` (run with Bun and
`--import tsx`). Native validators remain the runtime authority. No TypeScript
backend is called by these routes.

The optional provider-specific `get_hosting_status` tool and
`GET /api/registry/projects/{id}/hosting` are not implemented by this slice;
their provider behavior belongs in an installed integration. This document does
not claim generic native runtime observations replace that provider contract.

## Hosting

Resource routes translate the retained path targets into existing validated
native commands: apps (including deletion/storage previews, lifecycle, logs and
reconciliation), project app lists/creation, cluster inventory and registration,
activation/revocation, customer limits/usage/project lists, operation detail and
project creation/update. Path target fields cannot be overridden in the body.
Queued responses retain status `202`. Logs are read operations even though their
resource request uses POST. Feature disable/drain admission is repeated by the
domain under its installation fence.

The 31 implemented native command/tool mappings are declared once in
`resource_hosting::TOOLS`. Tool input schemas come directly from
`webdock_hosting::contracts::command_document()`, including the native Docker
extension, and remove only the server-owned `action` discriminant. Discovery
exposes only current read/write scope and enabled feature capabilities.

Still absent from this bearer/MCP slice:

- `clusters.enrollment` / `create_cluster_enrollment` and
  `projects.overview` / `get_hosting_project_overview`.
- `projects.relocate` / `relocate_hosting_project`.
- `setup.*` and `services.*` command/tool families.
- `byok.*` provider lifecycle and enrollment families.
- `git.*` source, build, release, target and connection tools.
- `databases.*` product, plan, offer, pool and migration tools.

The generic `POST /api/hosting/commands` path retains its family restriction to
`byok.*`, `git.*` and `databases.*`, but returns honest unavailable responses
until each family has a bearer-authorized native adapter. Existing browser/domain
implementations for some of these capabilities do not count as public API parity.
Unsupported capabilities are not advertised as working tools. Plugin tools are
shown only to Registry principals; Hosting customer tokens cannot discover them.

## Local verification

`cargo test -p webdock-control-plane --test resource_api --locked` checks the
unauthenticated HTTP boundary. With the explicitly owned local restricted-role
fixture environment, add `-- --include-ignored --test-threads=1` for actual
OAuth PKCE/consent, bearer API and MCP tests. They cover disjoint scope/tool
sets, native operation without plugins, Registry mutations/audit, token
revocation, live role/membership removal, mixed-scope rejection and expiry during
a write before commit. Six deferred-trigger cases pause actual Registry/Hosting
COMMIT and verify resource disable, issuer unlink and introspector unlink wait
until the mutation commits. These cases use private fixture resource identifiers,
not shared resource policy. Only uniquely named owned fixture rows are created and
removed. The existing plugin-host PostgreSQL acceptance test separately verifies
that plugin HTTP/MCP/private RPC behavior remains compatible.
