> Historical setup record. Billing and the Polar integration were removed from active composition on 2026-10-17; the instructions below no longer configure a supported feature. Retained database data is unchanged.

# Polar production setup

Organization: **spitzli**, ID `900da77a-d11a-4230-873b-e93fbb89b377`.
API: `https://api.polar.sh`. This configuration uses **live production**, not sandbox.

## Implementation

The Bun monorepo already uses TypeScript, Bun/Elysia in `legacy/control-plane`, and `@polar-sh/sdk` 1.0.2. No adapter or additional SDK dependency was installed. The installed SDK's current versioned `createPolar` and async `webhooks.validateEvent` APIs are used directly.

- `legacy/control-plane/src/modules/billing/hosted-client.ts`: one reusable SDK client selected by `POLAR_SERVER`.
- `legacy/control-plane/src/protocols/polar-hosted.ts`: `GET /checkout?products=<id>` creates a hosted checkout; `POST /api/webhook/polar` verifies the original signed bytes before acknowledging events.
- `legacy/control-plane/src/index.ts`: mounts these routes when the billing domain and environment token are configured.
- `legacy/control-plane/tests/polar-hosted.test.ts`: product validation, sanitized failures, raw signature verification and tampered-body rejection.
- Private, ignored `legacy/control-plane/.env`: `POLAR_ACCESS_TOKEN`, `POLAR_WEBHOOK_SECRET`, `POLAR_SERVER` (names only).

The hosted checkout has no custom success URL or confirmation page. No new customer-portal route was added: Polar hosts the portal and emails customers its link. Existing Webdock agreement/subscription functionality remains separate. The new `order.paid` and `customer.state_changed` handlers intentionally contain TODOs and do not grant access. Keep this distinction when testing a standalone checkout.

## Provisioned resources

- Product: **Test Product**, one-time **10 EUR**, ID `198f8368-fad6-4da9-bf64-b066b2c9a23f`. EUR replaces the requested USD price with owner approval because the organization requires its EUR default presentment currency.
- Webhook: `6556fb0b-ef66-4d12-9e68-65860bc8b44f`.
- URL: `https://api.webdock.dev/api/webhook/polar`.
- Raw events: `order.paid`, `customer.state_changed`; API version `2026-10`.
- 100% test discount: `10f71467-d9b6-42c4-9c29-daf8ba1bc5c9`. Restricted to the test product, one redemption, expires 24 hours after creation. Its code is stored in the private provisioning record, not source control.

Local checkout: `http://localhost:3130/checkout?products=198f8368-fad6-4da9-bf64-b066b2c9a23f`.
Production checkout: `https://api.webdock.dev/checkout?products=198f8368-fad6-4da9-bf64-b066b2c9a23f`.
Enter the test discount on Polar's checkout page; the plain checkout link does not apply it. Without the discount this is a real paid checkout.

## Verification before merging

- [x] Organization access token read from `.env`; product-list request returned 200.
- [x] Organization ID matches the owner-supplied production organization.
- [x] Existing Webdock connection probe passes locally and in the production API container.
- [x] API typecheck/build and all 55 API unit tests pass.
- [x] Signed order/customer event tests pass; tampered payloads return 403.
- [x] Product and webhook IDs were returned by Polar; webhook signing secret was saved directly to `.env`.
- [x] Deploy the new API artifact with the three private environment keys.
- [x] Production checkout redirects with 302 to Polar; unsigned webhooks return 403.
- [ ] Complete a hosted checkout using the 100% code and verify the resulting webhook deliveries.

A Python urllib probe was rejected by an edge browser-signature rule (403). The Node.js request and the installed SDK, including the production-container connection probe, succeeded. Do not diagnose that specific 403 as insufficient token scope.

## Local development

Use `bun run --filter @webdock/api dev` with the existing local database configuration and `WEBDOCK_API_DOMAINS` including `auth,portal,registry,billing`. The full Webdock API also requires its usual DB/auth configuration; the three Polar keys do not replace it. No long-running dev server was started by this setup.

## Webdock catalog

The original operator-authorized catalog contained 35 draft entries from the owner-provided product plan: websites, apps/bots, Connect/BYOK, mail, media/editor add-ons, databases and services. The owner subsequently removed the separate product-catalog module subscription: the product catalog belongs to the website package. The current source inventory has 17 fixed-price entries and 13 quote-only entries. The owner also removed hourly development and the two time-based change/maintenance allowances; all four removed subscriptions/products remain archived for audit history. The three website prices are yearly and include tax:

| SKU | Annual price incl. tax | Websites | Editors | Media | Emails/month |
|---|---:|---:|---:|---:|---:|
| website-mini-year-v1 | EUR 58.80 | 1 | 1 | 500 MB | 100 |
| website-basic-year-v1 | EUR 118.80 | 1 | 3 | 2,000 MB | 1,000 |
| website-plus-year-v1 | EUR 238.80 | 1 | 5 | 5,000 MB | 3,000 |

All 17 fixed-price entries are mapped as ready in both Polar environments through the operator workflow. Webdock sales and enforcement remain disabled. The earlier one-time Test Product is now archived. The live organization connection has also been saved successfully through the authenticated Webdock operator API.

The current source inventory is `legacy/control-plane/src/modules/billing/initial-catalog.ts`. Non-website prices follow the source's **net EUR** basis. Price ranges, starting prices and unspecified amounts are quote-only, never zero-price products. Service descriptions preserve the original capacity/backup/SLA limits. Additional websites reuse a website package; customer-specific sponsorships and individual plans are not sold as reusable free catalog products.

Migration `0021_full_catalog.sql` adds services, one-time intervals and nullable prices for quotes. Manual fulfillment is explicit; the existing SQL purchase function remains website-only. Quote-only entries cannot be published to Polar. No additional customer resources, service grants, sales enablement or operational approvals are created by seeding the catalog.

The initial production seed contained 35 catalog SKUs and 13 null-priced quote entries. After publication and the owner’s correction, each environment has 17 ready mappings, five archived products and 13 quote-only entries. Existing website rows were preserved; the other 32 were created through the authenticated operator endpoint. The API and worker use image `sha256:a981aa197a0e2b60912294a49dda29887ddbf6731d755fd05d7be6c4b9eb6fe7`. Restricted-role MCP catalog persistence, existing checkout and private-offer regressions passed.

## Publication read-after-write fix

Polar returned complete product-create responses while immediate GET/list requests still omitted the products. The same IDs became readable later. Publication now strictly validates the full create response (organization, metadata, price, currency, recurrence and tax behavior), rather than requiring an immediate GET. Uncertain/lost create responses still use inventory recovery and never automatically create a second product.

A diagnostic retry created one sandbox duplicate; it was archived and marked as replaced by the original. The removed `catalog-module-month-v1` subscription is archived in both environments; it is not an extra website charge. No payments or customer resources were created.

Regression coverage reproduces a successful create with a GET returning 404 and retains the lost-response/idempotency checks.

## Product translations

Webdock displays known catalog names and descriptions in the selected German/English locale using the shared Gettext catalog. Exact source-text lookup leaves custom or revised text unchanged. Signed agreement terms remain unchanged.

At the owner's request, Polar names and descriptions contain German and English together (identical brand names appear only once). Existing product IDs were updated in place; no language-specific products were created. The shared provider rendering and strict publication validation use the same bilingual text. Prices, recurrence, IDs and metadata were preserved and checked. Independent list/GET verification confirms exactly 17 active products in each environment after the removed time-based offers were archived.

The standalone domain-connection setup fee is also archived in both environments and removed from the source inventory: it is a human setup service, not a software subscription. The final active Polar catalog has 17 products per environment.
