# Polar billing operations

Billing runs in the Bun API and its restricted worker. PostgreSQL owns the agreement, inbox, jobs and service projection; Polar supplies verified financial facts. Studio is the native operator/customer UI. [Verification evidence](../verification/2026-10-14-api-rewrite.md) distinguishes local simulated tests from the owner's real sandbox organization.

## Connect an environment

Open Studio → Settings → Billing. Sandbox and live have separate organizations, tokens, webhook secrets, mappings, customers, subscriptions, orders and jobs. Use an **organization access token** for exactly one organization, with product/customer/checkout/subscription/order read access plus the write permissions required for catalog, checkout, subscription changes, webhook setup and customer portal sessions. The real sandbox test proved that OAT create requests must omit `organization_id`; every response is still checked against the stored organization.

The connection check stores the token encrypted and displays only its suffix. It proves reads, not successful payment setup. New sales, publication and live service enforcement are independently disabled by default. An environment becomes locked to its organization after use. Do not replace its organization to reuse financial mappings.

Configure a dedicated raw webhook endpoint at `/webhooks/polar/sandbox` or `/webhooks/polar` on the canonical HTTPS API origin. Required API version:2026-10. Event set: `customer.state_changed`, `subscription.created`, `subscription.updated`, `order.paid`, `order.refunded`, `checkout.updated`, `checkout.expired`, `product.updated`. Existing endpoints may be attached only after exact URL/org/version/events/secret verification; new endpoints are read back before use. Rotation accepts the previous key for at most24hours. An uncertain create is reconciled by complete endpoint inventory; it is never automatically re-created.

In Polar customer-portal settings, disable customer-managed plan, seat, unit and pause changes. Preserve invoice/payment-method/cancellation access. The API checks these controls and the current customer/org before issuing a short-lived portal URL, then rechecks current membership. A root read of another customer's billing does not mint that customer's portal session.

## Publish and purchase

Create an explicit product draft with commercial description/terms, integer minor-unit amount, EUR or CHF, tax display, monthly/yearly interval and a one-website entitlement snapshot. Mark its operational readiness and visibility explicitly. Internal products cannot be purchased. Publishing reserves an operation, writes once and validates the actual immutable product/price/trial/organization/metadata response. Unknown write results stay reviewable and are recovered by readback. There are no automatically seeded live prices or sales.

The archive control first blocks new Webdock purchases, then archives and reads back the same provider product. It never deletes a price, agreement or subscription. After a lost archive response, confirmation reads the already archived object without another update; an unconfirmed archive stays visibly retryable.

Changed prices or included services require a new version. Public catalog shows the latest ready version per SKU; previous accepted agreements retain their frozen version. Private offers have a dedicated private product/version, hashed expiring token, exact customer and current manual-plan revision. They are not reusable manual plan templates. A customer must already be an admin of that workspace and choose a ready, enabled managed website. Preview sessions cannot accept offers, check out or manage payments.

The customer reviews immutable terms and accepts explicitly. Acceptance and its single checkout attempt are committed before a provider call. Double-click/retry returns or recovers that same attempt. The browser success redirect never grants service. New callbacks go through `/api/billing/return`, discard any provider session token and preserve sandbox/live in the destination. The payment link, portal URL and raw provider token must never be copied into ordinary logs.

## Confirmation and service ownership

Webhooks verify the raw signature before storing the encrypted body and an inbox job atomically. Duplicate identical delivery returns200; a conflicting body is quarantined; accepted new events return202. A worker rechecks the stored hash/signature, fetches current provider state and reconciles all pages of subscriptions/orders/checkouts with a customer generation and claim token. Delayed/out-of-order webhooks cannot overwrite a newer read.

An active subscription alone is insufficient: the matching paid initial order is required. Live projection writes an absolute base under the existing plan lock, preserving extras and history. Revision changes only when the effective base changes. Sandbox confirmation simulates service state but never writes live tenant quotas or live claims. Provider outage preserves the last confirmed state. Partial refunds do not automatically cancel service; absence from a list is not cancellation.

Upgrades and downgrades are amendments to the existing subscription and agreement history. A customer accepts a frozen change quote with a UUID operation ID. Mixed-dimension/currency/interval changes require review. Upgrades invoice now and wait for a new paid amendment invoice. Downgrades schedule the next exact period boundary; the old allowances remain until the confirmed boundary. An uncertain update is read back by operation metadata, never blindly repeated. Only one unresolved amendment exists per subscription. Historical prices remain valid for historical orders.

Paid bases fence manual-plan overwrite, including old/raw writers; extras remain editable. Cancellation does not automatically restore an earlier manual plan. An explicit root manual release requires a closed provider agreement, no outstanding invoices, matching customer name/revision and an audit reason. Only then may the operator assign a new manual base. Financial history continues to block destructive customer/project cleanup. There is no automatic seven-day cutoff, financial-data deletion or unapproved retention/grace policy.

## Worker and recovery

Run the shared image with `bun dist/worker.ts`, only worker credentials, `SPITZLI_WORKER_DOMAINS=billing` and explicit `SPITZLI_BILLING_WORKER_ENVIRONMENTS=sandbox` or reviewed `sandbox,live`. The worker uses PostgreSQL leases/CAS and bounded retry delays; stale workers cannot finish another claim. A process restart reclaims only expired leases. Sales/publication disabled does not stop webhook receipt, reconciliation or termination processing.

Studio Billing shows pending/review counts and oldest queue/reconciliation times. Retry controls only reset read operations (`webhook`, `reconcile`, `catalog`); receipt conflicts, unsupported versions and missing bodies remain quarantined. They cannot repeat checkout/product/endpoint/subscription writes. Use the customer billing review to trigger a fresh read. Inspect provider facts before resolving an ambiguous write; preserve its operation IDs and original acceptance. Do not delete a job or create a second checkout as a retry mechanism.

## Acceptance and rollout

Local disposable role tests cover tenant/preview isolation, stale leases/generations, replay/conflicting webhooks, lost write responses, paid-order gating, invoice failure, scheduled downgrade boundary, manual release and one-subscription amendments. The real sandbox exercise uses a separate local database and a temporary HTTPS receiver exposing only the signed sandbox webhook route. It does not certify production Auth/Studio TLS ingress or host capacity.

For a live pilot: complete [API cutover](api-cutover.md), verify backup/restore and pool budget, verify the live organization read-only, explicitly approve real products/prices/terms, then enable a named pilot customer and sales. Enable service enforcement only after observing confirmed records and testing cancellation/recovery. Never copy sandbox mappings or encryption contexts into live.

[Polar sandbox documentation](https://polar.sh/docs/integrate/sandbox) confirms environment isolation and test-card behavior. Sandbox email delivery is restricted by Polar to organization members; the rehearsal used a synthetic non-member address. Test products are clearly named and archived after the exercise; the temporary endpoint is disabled before tunnel shutdown. No live payment or customer was created.

## Operator MCP catalog

The existing Studio MCP endpoint exposes `list_billing_product_versions` for current operators with `webdock:read`. `webdock:write` additionally exposes `create_billing_product_version`, `publish_billing_product_version` and `archive_billing_product_version`. Each call rechecks the current central session/MFA; Hosting scopes do not grant catalog authority. Environment selection is explicit. Amounts are integer minor units; media bytes are decimal strings. Draft creation does not publish or charge anyone. Publication is a separate guarded call, so an operator can create and sync an approved onboarding product in one instructed workflow. Connection keys and publication switches stay in the native Billing settings, not MCP tool inputs.
