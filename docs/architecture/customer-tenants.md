# Customer tenants

Studio's `webdock_admin.customers` remains the canonical customer profile. It contains person/company selection, names, contact information and postal address. `notes` remains operator-only. Saving a contact email never grants account access.

`webdock_auth.tenant_customer` maps each customer ID to exactly one Better Auth organization ID. Existing organizations are mapped explicitly, not matched by email or mutable names. The initial production mapping preserves Spitzli Development and Stall Eichenbruch and their existing website bindings and memberships.

A database trigger provisions an organization and mapping in the same transaction as each new customer. Updating the customer display name synchronizes the organization name. Rolling back the customer operation rolls back the organization too. The trigger function is security-definer, uses a fixed `pg_catalog` search path and fully qualified relation names, and has no public execute permission. Studio has no direct auth-table privileges.

The central auth runtime receives only named SELECT/UPDATE permissions for public customer fields. It cannot read internal notes, change customer status, or modify IDs. Tenant isolation between rows is enforced by the auth service on every operation; the SQL column grants are an additional boundary, not a replacement for membership authorization.

## Customer flow

1. An operator creates a customer in Studio and enters contact/address information.
2. The customer's **Tenant & invitations** link opens `auth.webdock.dev/tenants/{customerID}`.
3. An operator invites an initial customer administrator. New invitees receive the existing password-setup flow; existing accounts receive an organization invitation.
4. The invitee proves email ownership, sets up their account and accepts the invitation. Pending invitations alone provide no membership access. Public signup remains disabled.
5. Customers use **Tenants** in their account navigation to access their own active tenants. Profile changes do not alter account email addresses or membership.

Tenant membership uses Better Auth's owner/admin/member roles. The platform operator role is separate. Tenant administrators can manage the profile and invitations; changes to existing roles and membership removal are operator operations. Operators may suspend accounts as a security response even if that temporarily leaves a tenant without an active customer administrator. Normal demotion/removal protects the last customer administrator.

Owners and administrators of an active mapped customer tenant receive CMS administrator access to websites bound to that same organization, without an extra per-website grant. Ordinary members still require an enabled per-website grant and receive its assigned role. Explicitly disabled/revoked or mismatched grants deny access even for tenant administrators; enabled lower-role grants do not reduce their inherited administrator role. Unmapped organization membership grants no implicit CMS access. Studio's global portal binding remains a reader identity, never platform operator access. Archived customer tenants, suspended accounts, unverified accounts and unfinished account setup cannot access the tenant portal; mapped archived tenants also lose CMS authorization through live claims checks. Disabled bindings, disabled OAuth clients and the CMS availability switch also block customer CMS access; operators retain their existing security requirements and maintenance access.

## Deployment

Apply the admin Payload migration `20261004_081809_customer_profile` before deploying code that reads the new fields. Then run the offline bridge migration with an owner credential file and a JSON object of existing customer-ID to organization-ID mappings:

```
bun apps/auth/scripts/migrate-tenants.ts /secure/owner.env /secure/existing-mappings.json
```

The runner refuses conflicting mappings and ambiguous unmapped existing organization names. Owner credentials remain local and are never added to deployed apps. The runtime role granted public profile access is `webdock_auth_runtime`. The migration is transactional and repeatable; keep the additive schema in place when rolling back application code.

Regression coverage includes customer profile round trips, atomic tenant creation/name synchronization, restricted profile column privileges, tenant-bound reads and mutations, and invitation/membership permissions. Tests use the disposable local `webdock_admin_test` database, never real customer invitations.

## Next: Mail

The requested Mail module will use turboSMTP behind the product-facing name **Mail**. Provision provider subaccounts and sender domains per tenant, retain provider identifiers and credentials with explicit tenant ownership, expose DNS records/verification state, and require tenant authorization for every provider operation. The implementation and remaining live-provider/domain setup are described in [Platform administration and Mail](platform-mail.md).
