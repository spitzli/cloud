# Billing runtime and platform persistence boundary

Source checkpoint: 2026-10-15. No migration, production identity, provider resource, billing contract or encryption key changed.

## Platform persistence

`packages/platform-db` owns the existing `webdock_auth` authentication/session mappings, Billing tables and `webdock_admin` registry mappings. The three schema definitions were copied intact, including physical schemas, column types, Snowflake defaults, constraints and indexes. Existing `apps/control-plane/src/db/schema/{auth,billing,registry}.ts` paths remain compatibility exports. This is schema ownership in source, not a database migration or a new ORM.

The package depends on Drizzle and validation primitives, not applications, product modules or core services. Its typed persistence ports expose the existing Drizzle transactions and restricted worker query/connection handles; pool creation and signed RLS context remain controlled by API composition. Decimal Snowflake IDs remain strings with the same signed-64-bit bounds. Core Registry can consume these mappings without a package dependency cycle.

## Module runtime

`modules/billing/src/server` now owns the actual catalog, immutable product versions, connection settings, endpoints, offers, checkout, customer portal, subscription changes, manual handover, reconciliation, webhook inbox/dispatch, job leases and worker scheduling behavior. Pure policies, cryptography, initial catalog and RLS policy generators are also module-owned.

Each stateful file exports an explicit factory. The control plane composes its persistence handles, cryptography, provider ports, canonical origin settings and installation admission checks at the existing API path. There is no mutable global dependency registry and no module import of an application. All environment reads stay in API composition and are read at the same operation boundary as before. The existing public function signatures and compatibility exports remain available to routes, MCP, workers and tests.

Authorization still uses the local subject/session reference and the existing signed PostgreSQL policies. Operator and customer-admin authorization is checked before and after transactional work. Live/sandbox isolation, quotation hashes, expected revisions, one-use offer and checkout bindings, endpoint identity checks, operation journals, leases and uncertain-write reconciliation fences are preserved. New purchases and subscription upgrades still invoke the installation admission port; existing accepted work retains its previous handling. Moving the source does not turn provider failure into an automatic repeat of an uncertain external write.

## Polar adapter

`adapters/billing/polar` owns construction of the existing official `@polar-sh/sdk/2026-10` client, hosted client lifetime and webhook signature verification. Provider origins and API version remain fixed; bound environment/token arguments are explicit. The module keeps the official versioned provider DTO/method types through a type-only contract, with no SDK runtime import or provider initialization. This preserves strong existing types rather than hiding provider differences behind an unimplemented generic payment abstraction.

Webhook verification returns explicit verified, invalid-signature and malformed outcomes. Correctly signed unknown event types retain their payload for the existing quarantine path. The module still owns bounded raw-body reading, environment checks, receipt hashing, duplicate/conflict handling and transactional inbox/job persistence. Upstream errors and credentials remain absent from public errors.

Billing encryption retains its existing root-key source and authenticated context: environment, secret kind and version. No key is generated, rotated or rebound by this extraction. Hosted checkout remains a separate compatibility path with its existing configuration and URL behavior.

## Billing UI and admission

`modules/billing/src/ui` owns the real tenant Billing page, operator customer review, admin connection/webhook forms and product catalog presentation/forms. The filesystem pages retain authentication, data loading and redirects; the existing server actions keep validation, cookie authentication, revalidation and provider redirects. Presentation receives public DTOs, action ports, labels, catalog-family data and Link rendering explicitly. Public Billing DTOs are module-owned at `./contracts`; `@spitzli/api-client/billing` is a type-only compatibility export, avoiding a reverse module dependency on the API client.

Action-state boundaries include tenant, environment, resource and revision/quote identity. Private-offer and sandbox customer pickers also remount their action-result state when the selected customer changes. A generated offer URL cannot remain visible under another selected customer or environment. Core plan/usage screens and the manual-plan offer workflow remain independent of optional Billing; they were not moved into the module.

Direct API composition now supplies the existing installation-operation guard before catalog creation/publication and private-offer issuance. Disabled/draining installations reject these admissions before database or provider I/O. Archive/revoke, reconciliation and existing management paths do not acquire a provisioning guard. Checkout and subscription-upgrade admission ports remain in place; downgrades retain access semantics.

## Evidence and limits

The control-plane typecheck passes. Thirteen existing Billing/hosted compatibility tests passed with pinned Bun 1.4.3. Thirteen new offline module/adapter/platform tests passed, covering authorization rechecks, signed scope lifetime, absent credentials, lease fences, redacted retry/review codes, encryption context, real SDK webhook verification/tamper refusal, unknown-event quarantine, schema identities and exact Snowflake bounds. Two Console rendering tests also pass, including a real React action-result reset across customer and environment changes. Provider HTTP tests inject responses and make no live provider calls.

Full PostgreSQL Billing integration tests continue through the parent's centrally owned isolated RLS fixture. This task did not create or tear down fixtures, call live Polar, publish a release or alter production billing. Source ownership does not imply a new provider or interchangeable production billing semantics.

New cloud services and AWS/GCP additions are explicitly out of scope by the user’s selection; this work completes the existing provider boundaries.
