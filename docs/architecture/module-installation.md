# Installation modules and product boundaries

Implemented against the Bun Control Plane and Next.js Console. This document describes installed-module behavior. Authentication, package ownership and self-host distribution are implemented in the accompanying boundary documents; no release or production rollout is implied.

## Runtime configuration

Set `WEBDOCK_MODULES` on the **API and worker**, using the same value on every replica. The Console receives public state and its deterministic revision in the authenticated Studio session; no build-time public environment variable or independent Console setting is used.

```json
{"configVersion":1,"modules":{"mail":{"enabled":false}}}
```

The static registered modules are `mail`, `hosting`, `databases`, `cms`, `storage` and `billing`. Unknown module IDs, unknown fields, unsupported config/SDK versions, malformed state and missing/cyclic dependencies fail validation. An explicit configuration disables omitted modules. An absent variable preserves existing installations; it does not create accounts or override existing product entitlements. An empty variable is an error.

The `state` form supports the transition lifecycle:

```json
{"configVersion":1,"modules":{"mail":{"state":"draining"}}}
```

Use either `enabled` or `state` per entry, never both. `enabled:true` is equivalent to `state:"enabled"`.

`WEBDOCK_API_DOMAINS` and `WEBDOCK_WORKER_DOMAINS` still select which processes own routes and jobs. Module activation never grants process ownership, customer entitlement, membership or project access. The Console hides products if the API does not own their domain (`storage` uses `portal`). A worker assigned to any disabled or unknown product fails before opening its database pool. Shared project/cluster management bindings, internal registry and the platform PostgreSQL database remain core services independent of customer CMS and database products. Shared authenticated hosting-agent transport remains available to reconcile database and Mail resources; customer hosting routes are separately gated.

## Safe shutdown

1. Roll all API and worker replicas to `draining` and stop old replicas that still admit new accounts. Existing resources, message delivery, callbacks, reconciliation and management continue. New Mail accounts/connections, domains, mailboxes and first native service activation are refused. Resuming an existing suspended native instance remains allowed.
2. Explicitly delete or hand over external resources using their existing authorized workflows. Suspension alone retains the native instance and is insufficient. There is no automatic handover or destructive cleanup in this feature.
3. Change all replicas to `disabled` and remove Mail worker ownership. API startup checks retained native services, legacy accounts, BYOK credentials, active connections/domains and unfinished provider/message, callback, batch, webhook and native outbox operations using the existing signed, installation-wide Mail worker scopes (`mail-cron` and `mail-send-gateway`). Any retained resource, incomplete query or database error aborts startup. No provider is called and no row is deleted by this preflight.
4. Keep the Mail encryption key, schema and retained storage. Schema migration is independent of activation. Re-enabling a module does not move or rebind existing resources.

This is a restart/rollout configuration, not a live feature toggle. The preflight is not a distributed fencing mechanism: old enabled replicas must be stopped before disabling. Existing system authentication email is separate and remains available when the customer Mail module is disabled.

## Package boundaries

- `packages/module-sdk`: provider-free manifests, strict configuration, registry, public states and React context/hooks. Console applies tenant, management and preview restrictions to navigation contributions.
- `packages/core`: local identity/session/membership contracts and internal customer/project registry. `packages/platform-db` owns the existing physical schema mappings; optional customer Databases does not own platform persistence.
- `packages/console-kit`: shared presentation primitives, navigation state and queries, with no provider credentials.
- `modules/{mail,hosting,databases,cms,storage,billing}`: product services and UI behind typed ports. Console wrappers own authenticated server actions; Control Plane wrappers compose transactions, identity and adapters.
- `adapters/`: Better Auth, Clerk, turboSMTP, Vercel hosting/storage, GitHub, PostgreSQL and Polar implementations. Credentials enter at server composition, never public manifests or UI DTOs.
- `integrations/tabularis-web`: customer launch validation and gateway capability policy; the actual vendored UI and separate gateway runtime retain their existing isolation model.
- `agents/kubernetes`: independent Python execution and reconciliation; module selection grants no new cluster privileges.

Existing API import paths, schemas, local IDs, secret bindings and OAuth/MCP/Studio URLs remain compatible. There are no reverse application imports from product implementations. Shared provider and runtime contracts prevent module dependency cycles.

The selected browser adapter supplies current identity to product authorization. Clerk uses explicit immutable mappings and short-lived local session projections; legacy protocol authentication remains separate. Durable user-attributed work rechecks current user/grant authority without requiring the original browser session to stay alive. Read `clerk-browser-auth.md` for the exact worker and OAuth refresh boundaries.

All six products are gated at their applicable API mounts, direct paths, Studio operations, provisioning services, worker selection and MCP calls. Shared agent transport and internal registry remain core. An older API without public module state fails closed in Console; roll the API first.

## Local verification

```sh
bun run test:modules
bun run --filter @webdock/api test:unit
bun run --filter @webdock/admin test
bun run --filter @webdock/api check
bun run --filter @webdock/admin check
```

The existing API fixture scripts provision isolated loopback Docker PostgreSQL databases for provider/Studio regression tests and restricted-role preflight tests. They must not be pointed at customer databases. Provider regressions inject responses and block external network access.


The public operator/developer guide is `docs/developers/modules.mdx`, linked from the developer navigation and Mail guide.

## Other product shutdown checks

API and worker startup check disabled products independently of process-domain ownership. A dedicated signed `installation-preflight` worker scope receives installation-wide SELECT only on an explicit resource-table allowlist. It cannot insert, update or delete using this purpose. Each check verifies that its SELECT policy is installed before trusting an empty result. Deploy the RLS migration before selecting disabled products. The checks use the worker pool for both `webdock_auth` and the `webdock_admin.cms_instances` inventory, never a tenant-filtered registry session.

Hosting checks retained applications/volumes, provider bindings, application reservations, setup records, Git connections/build/release/outbox records and unfinished operations. Databases also checks setup configurations selecting a new or existing database, including queued work before a resource exists. It checks non-purged databases/backends/migration sources, orders, delegated bindings/grants and unfinished operations. Storage checks all managed store credentials. CMS checks registered instances. Billing checks provider credentials/endpoints, contracts/subscriptions/checkouts/changes and unfinished jobs/events.

These checks intentionally retain conservative blockers for historical setup, order, Git and billing records: an operator must resolve or explicitly hand over them before disabling. There is no automatic cleanup or override. A successful startup is not proof that resources outside the registered inventory do not exist. Suspending resources remains insufficient; re-enabling preserves identities and bindings.

Draining refuses new application/database provisioning, setup confirmation, provider connections and billing purchases/publication. Existing start/resume/reconcile/delete/purge commands remain accessible with their usual permissions. Internal queued setup execution must retain its existing authorized continuation rather than re-entering a new-provision ingress. Customer database UI and delegated database endpoints are unavailable when `databases` is disabled; the platform database and registry continue operating.
