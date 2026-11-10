# Common Core, auth and Managed Apps release

User explicitly authorized integration, release build, migration, production rollout and existing Tabularium app adoption on 2026-10-18. Preserve existing data/services, locally built/EU runtime artifacts and configured signing. No repeated authentication retry loops.

1. Audit source and production. Local source audit found all tracked changes and non-cache new source relevant; exclude .cache credentials. Preserve origin/main documentation corrections when merging.
2. Create clean reviewed source checkpoint, merge origin/main, run appropriate unit/contract/isolated PostgreSQL checks and product-free/full builds. Independent full integration review before merge.
3. Existing production retains Hosting/Mail/DB/Storage: deploy compatible full API/worker/Console; distribute api-core/console-core separately. Do not force pure core inventory cleanup or delete data.
4. Back up exact runtime specs/secrets and database/policy state privately. Rehearse new RLS against an isolated restored database. New worker requires a private persistent noeviction Redis queue; existing Dragonfly view cache must not be silently reused.
5. Verify current production Clerk identities/approved operators explicitly. Current Console uses Clerk via legacy selector, raw SPITZLI_AUTH_PROVIDER absent. Configure unified selection consistently only with explicit operator mapping and provider metadata review. Retain OAuth/MCP compatibility as required. No role inferred solely by email or stale local role.
6. Build/stage matched API/worker and full frontend artifacts. Preserve Vercel project IDs/upstream secrets; update renamed app roots carefully. Do not apply generic Helm network policies to live topology.
7. Coordinated maintenance: drain/stop old writers, install matching RLS helper signatures via bounded offline migrator, start matched releases, verify readiness/login/roles/retained product flows. Restore matched previous code+policies on failure, not code-only rollback.
8. Install verified Managed Apps alpha, bind only existing app104651089088876544 in project104618288083894272, preserve dedicated Tabularium and shared analytics. Verify read/cross-project denial/launch links; do not stop live registry just for a smoke test.
9. Revoke temporary migration privileges, retain safe recovery evidence and report actual release status/limits.

Release decisions confirmed during preparation:
- Owner chose the existing mixed mode: global Better Auth, native Clerk Console, Better Auth OAuth. Full Clerk conversion is deferred; no MFA requirement is weakened.
- Console and Auth use local prebuilt deployments with explicit promotion. Disable automatic Git deployment so frontend activation cannot race the coordinated RLS/API update.
- The existing platform quota prohibited all PVCs. Permit exactly one 1 GiB claim for the persistent jobs queue, and raise aggregate limits by the queue's required CPU/memory capacity. Existing applications and quotas elsewhere remain unchanged.
