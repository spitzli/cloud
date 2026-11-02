# PostgreSQL plugin (step 2) Implementation Plan

> Executed inline (owner: "ja dann bau mal die plugins neu! will das testen"). Spec: `docs/superpowers/specs/2026-10-21-product-plugins-design.md`.

**Goal:** A `postgres` plugin with shared databases and dedicated instances, a React UI, installed in local dev so the owner can test it end to end.

**Architecture:** Standalone Rust workspace in `plugins/postgres` on the existing plugin runtime, with its own PostgreSQL database for state. Shared databases live on operator-configured shared servers (role + database per resource). Dedicated instances are containers created by the plugin's local Docker placement; the production placement through `plugin.workloads` and the agent replaces it in a later step. UI: `plugins/postgres/ui` React package that the Console bundles and shows only when the plugin is installed.

## Global Constraints

- No product code in Core or in the Console's own source. Plugin UI lives in `plugins/<id>/ui`; the Console finds it by a generic glob.
- No prices. Default access mode `assigned`.
- No secrets in repo files, logs or commits. Credentials at rest are encrypted with a key from the plugin's private configuration.
- Local only: disposable containers. No SMTP to 127.0.0.1:1025.
- Bun only for JS tooling.

## Rulings (my calls)

- Installation-scoped operator actions stay deferred. The operator page lists all resources from the Core resource index (`GET /api/console/plugins/{id}/resources`); an operator opens a resource in its project like the customer does. Cost if wrong: an operator view without per-server capacity until installation scope lands.
- Shared servers come from the plugin's private configuration, not an operator form. Cost if wrong: adding a server needs a config change.
- Dedicated instances use a Docker placement in local dev only, marked as such; Kubernetes/CNPG goes through `plugin.workloads` later. Cost if wrong: dedicated instances do not work outside local dev yet.
- Plugin storage is a database URL in the private configuration, provisioned by the dev seed; no manifest field. Cost if wrong: one manifest field to add when the installer provisions it.
- The plugin UI is bundled at Console build time (first-party plugins only). Runtime loading arrives with third-party React plugins.

## Tasks

1. **Core: operator resource list.** `GET /api/console/plugins/{id}/resources` (operator only, not preview) returning the index rows with customer and project names. Test in `crates/plugin-host/tests/self_service.rs`.
2. **Console: native plugin UI.** `import.meta.glob` over `plugins/*/ui/index.tsx`; `@webdock/plugin-ui` SDK (invoke/query hooks, UI kit). The resource page and the extension page render the native component when one exists; otherwise the declarative panel. The extension page also gets a resource list across customers.
3. **Dev: every plugin installed.** `tools/dev/local.mjs` seeds a list of plugins (image in the local registry, release, installation, runtime, private config, own database and role). Tiltfile builds and runs each. Compose gets a `pg-shared` server.
4. **Plugin: postgres.** Offerings `shared` and `dedicated`. Features `create-shared`, `create-dedicated`. Actions: `databases.create`, `instances.create`, `databases.view`, `databases.credentials`, `databases.rotate`, `databases.delete`, `instances.delete`. Integration tests against a disposable local PostgreSQL.
5. **Plugin UI: postgres.** Resource page with status, connection details, reveal password, rotate and delete.
6. **Live run** in the local stack, docs, push.

## Status (2026-10-22)

Tasks 1 to 6 done. Verified: plugin tests against a disposable PostgreSQL and Docker (shared lifecycle, isolation between customers, rotation, capacity, dedicated lifecycle); Core test `operator_lists_resources_across_customers`; browser test `postgres-plugin-browser.mjs`; live run in the local stack (create, connect with the shown credentials, rotate, delete for both offerings, nothing left behind); `bun run quality` green.

Open: production placement for dedicated instances (`plugin.workloads` + agent/CNPG), TLS to shared servers, installation-scoped operator actions, removal of `crates/databases` and the Console database pages from Core, the Tabularis browser.
