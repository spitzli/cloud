# Neon database plugin and registry presentation

Status: source and fixture qualification; signed publication and live installation
are separate remaining gates. No production migration or Core rollout is recorded
by this report. The machine-readable companion records the individual results.

## Architecture

The independent `spitzli/cloud-plugin-neon` runtime provides existing-database
discovery, attachment, settings, branch inventory, explicit URI reveal and local
detach. Core retains the existing BYOK connection endpoint and stores encrypted
keys, attachment metadata and nonsecret replay receipts in its existing database.
The plugin has no database, provider key, SQL executor or direct Neon egress.

The frozen generic `HostClient.call(token, "http", "request", input)` ABI remains.
The optional named `neon` adapter uses persisted browser admission and fresh
customer/project/action/resource/grant/session/key checks. Legacy static HTTP
validation remains separate. Only existing database managers, authorized
operators and the existing read-only customer preview retain their corresponding
access. Ordinary membership gains no additional Neon permission.

The initial provider region is AWS Frankfurt (`aws-eu-central-1`). Auth, Storage,
Realtime, Functions, AI Gateway, database provisioning and automatic application
bindings are future capabilities. API-token/MCP/job invocation is unsupported for
this first browser release.

## Change inventory

| File or narrowly related files | Change |
| --- | --- |
| `crates/provider-connections/src/database.rs` | Fixed-origin broker, safe projections, immutable attachment scope, before/after checks and strict URI validation. |
| `crates/provider-connections/src/database_tests.rs` | Projection, route, URI override/multihost/encoding and region regressions. |
| `crates/provider-connections/src/lib.rs` | Compose the adapter and calculate management availability from current authorization. Existing connection permission remains. |
| `crates/provider-connections/src/neon.rs` | Expose actual provider project region to the scoped broker. |
| `crates/provider-connections/Cargo.toml` | Existing Host adapter dependency and isolated test transport. |
| `crates/plugin-host/src/http.rs` | Nonserializable admitted adapter context and bounded named requests; retain static validator. |
| `crates/plugin-host/src/authority.rs` | Derive context from persisted admission, preserve current authority and enforce connection-management scope for broad inventory. |
| `crates/plugin-host/src/self_service.rs` | Atomic local attachment receipt/replay integration and narrow abandoned local reservation recovery. |
| `crates/plugin-host/tests/neon_adapter_contract.rs` | Named-request validation boundary. |
| `crates/plugin-host/tests/neon_databases.rs` | Owned PostgreSQL upgrade/repeat, receipts, rollback, revocation, resource and tenant-boundary integration. |
| `crates/plugin-host/Cargo.toml` | Focused integration-test dependency. |
| `crates/persistence/migrations/0075_neon_database_attachments.sql` | Forced-RLS attachment/receipt tables and restricted local commit/reclaim functions. No provider mutation. |
| `crates/persistence/migrations/frozen.json` | Append the accepted 0075 checksum; retained entries unchanged. |
| `crates/control-plane/src/app.rs` | Reuse the Core-owned connection service in the optional plugin host. |
| `crates/control-plane/src/neon_connections.rs` | Share retained encryption-key selection between HTTP and private callback, with priority regressions. |
| `crates/control-plane/src/plugin_config.rs` | Retain the configured protocol issuer for private browser/OAuth validation. |
| `crates/control-plane/src/plugins.rs` | Optional adapter composition, browser-only release boundary and no-store URI responses. |
| `crates/control-plane/src/bin/webdock-plugin-control.rs` | Supply the real connection broker to the separately supervised callback listener. |
| `crates/control-plane/src/bin/webdock-plugin-controller.rs` | Keep the controller constructor explicit; no Neon action broker in the controller lane. |
| `crates/control-plane/src/bin/webdock-agent-control.rs` | Keep the agent-control constructor explicit; it is a different service from HostRpc. |
| `crates/control-plane/src/plugin_installation/worker.rs` | Stateless installation skips unnecessary database control privileges; stateful separation proof remains. |
| `crates/control-plane/tests/support/recovery_mail_component.rs` | Adapt existing test constructors to the optional dependency. |
| `Cargo.lock` | Local dependency edges; existing external versions/checksums retained. |
| `plugins/neon/{README.md,manifest.json,ui/index.tsx,ui/nav.ts,ui/tsconfig.json}` | Canonical independent-repository reference and Console compatibility UI snapshot. |
| `apps/console/src/plugins/sdk.ts` | Expose the existing i18n hook to the peer UI module. |
| `apps/console/tests/neon-plugin-state.test.tsx` | URI lifecycle, provider-outage detach, read-only and German/data-preservation tests. |
| `apps/console/tests/neon-plugin-fixture.{html,tsx,vite.config.ts,md}` | Explicit test-only actual UI/SDK/Console renderer with synthetic transport, customer principal and English/German modes. |
| `apps/console/tests/neon-plugin-fixture-serve.mjs` | Bounded loopback serving of the already EU-built fixture. |
| `apps/console/tests/neon-plugin-browser.mjs` | Customer selections, retry identity, reveal cancellation/cache isolation, detach, role controls and 320px checks. |
| `apps/console/tests/neon-plugin-capture.mjs` | Genuine unedited gallery capture with synthetic banner, hidden credentials, disabled writes and provenance hashes. |
| `packages/i18n/locales/de/neon-plugin.po` | German messages, preserving existing translations. |
| `packages/i18n/compiled/de.json` | Deterministic additional messages. |
| `packages/i18n/scripts/extract.py` | Include the specific new UI source root; existing POT preserved. |
| `tools/plugin-release/{catalog_presentation.py,test_catalog_presentation.py,README.md}` | Verify actual immutable-source icon/README/gallery publication, not just local files. |
| `tools/plugin-ci/BLACKSMITH.md` | Record the seventeenth selected repository and distinguish EU manual qualification from pending actual Blacksmith CI. |
| `plugins/README.md` | Catalog retirement and Neon source ownership. |
| `docs/guides/{byok-neon.mdx,databases.mdx}` | Current key/attachment workflow, permissions, limits and explicit capability boundaries. |
| `docs/operations/2026-11-02-plugin-catalog-presentation.md` | Live nine-entry repair and five selected removals with retained runtime identities. |
| `docs/operations/neon-plugin-rollout.md` | Private listener/controller/catalog prerequisites, controlled migration, compatible rollback image and customer acceptance. |
| `docs/superpowers/{specs,plans}/2026-11-02-neon-database-plugin*.md` | Scoped design, delivery gates and confirmed existing permission boundary. |

The independent repository owns its Rust runtime and tests, pinned SDK/toolchain,
Cargo locks, Dockerfile, strict startup/mTLS/Unix handling, Blacksmith workflow,
immutable release scripts, manifest validator, UI, licensing, official Neon icon,
README and three genuine screenshots with provenance. No new SDK wire version is
introduced. Its authoring zero image digest must never be published as a release.

## Infrastructure and release gates

The existing agent generates Namespace, immutable Secret, Deployment, Service,
NetworkPolicy and TLS-passthrough IngressRouteTCP for an admitted runtime. Neon
uses the stateless recipe; it needs no metadata service or persistent database
volume. Private Core callback/controller supervision and the actual target profile
must still be installed and tested through the existing managed path.

Before production DDL, qualify both the candidate and an old-behavior rollback
image that understands the accepted migration journal. The old 0072 image alone
fails strict readiness after 0075. Preserve all live key selection, existing
runtime state, provider data and immutable image/release identities.

A genuine browser capture verifies the Console renderer with synthetic data. It
is not evidence of a live provider account or a production managed installation.
MFA preservation coverage in the owned migration fixture concerns the operator
account and MFA flag; real operator login and retained credentials require the
controlled live acceptance step.

## Repeatable checks

Native/application compilation runs on the qualified EU build infrastructure.
The independent repository's `STANDALONE.md` describes its pinned check/release
procedure. The owned PostgreSQL fixture requires PostgreSQL18 and UID1001 in the
qualified disposable image; it accepts no ambient database DSN:

```sh
WEBDOCK_PLUGIN_TEST_LOCAL_POSTGRES=1 \
  cargo test --locked -p webdock-plugin-host --test neon_databases -- --ignored --nocapture
```

From the project root, local static/unit checks use the existing locked tools:

```sh
bun test --isolate apps/console/tests/neon-plugin-state.test.tsx \
  apps/console/tests/plugin-reveal.test.tsx apps/console/tests/plugin-panel-v2.test.tsx
bun --bun node_modules/typescript/bin/tsc --noEmit --project apps/console/tsconfig.json
python3 -m unittest discover -s tools/plugin-release -p 'test_*.py'
python3 packages/i18n/scripts/compile.py --check
```

For the already built fixture, use Node24 and the existing browser dependency;
these commands build no application and contact no provider:

```sh
NEON_PLUGIN_FIXTURE_DIR=.runtime-test/neon-plugin-20261102/frontend4-results/fixture \
  node apps/console/tests/neon-plugin-fixture-serve.mjs
NEON_PLUGIN_FIXTURE_URL=http://127.0.0.1:3138/tests/neon-plugin-fixture.html \
  bun apps/console/tests/neon-plugin-browser.mjs
```

If the package's default Chromium is not installed, set
`PLAYWRIGHT_CHROMIUM_EXECUTABLE` to an already installed compatible executable.
The recorded acceptance used Chromium156. Capture regeneration and the five
minute loopback server boundary are documented in the fixture guide.
