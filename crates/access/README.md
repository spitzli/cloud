# Native access service

`webdock-access` preserves the retained organization/member/invitation/project-grant model and uses the existing canonical IDs. All new row IDs use the injected dedicated PostgreSQL allocator. It does not create customers: use the registry service so the customer/organization allocator bridge is applied.

## Runtime integration

Construct `AccessService::new(business, auth, identity, allocator, signer, encryption_keys, config)`:

- `business`: `DATABASE_URL`, restricted `webdock_auth_runtime`.
- `auth`: `AUTH_DATABASE_URL`, restricted `webdock_auth_bootstrap`; required for invitation acceptance and account security mutations. Do not substitute a business pool.
- `identity`: shared `Arc<IdentityService>` with retained cookie policy, encryption ring, allocator and account origin configured.
- `allocator`: `Arc<IdAllocator>` using a dedicated pool.
- `signer`: `RlsSigner` for audience `webdock_auth_runtime`.
- `encryption_keys`: shared retained `Arc<SecretKeys>`; outbox payloads and migration archives use versioned authenticated encryption.
- `AccessConfig`: canonical `auth_origin`, all Console origins to exclude from website lists, explicit `allow_local_http` for local development only.

All browser methods accept the raw Cookie header; corresponding `_as(&Principal, ...)` entry points are for trusted backend resolvers and revalidate the current persisted native session. Never deserialize a caller-supplied Principal; a bare subject/session ID is never browser authentication. Errors provide `status()` for HTTP mapping. Apply CSRF/origin checks at the host router for mutations.

| Method | Result / behavior |
| --- | --- |
| `require_operator(cookie)` | Live complete local MFA operator principal; previews denied |
| `list_access(cookie, search)` | Bounded operator users, organizations, websites, grants, invitations, audit events; accent/prefix/typo search |
| `manage_access(cookie, AccessCommand)` | Create organization, link website without ownership transfer, invite, change/revoke site grant, suspend/unsuspend nonoperator account, cancel pending invitation |
| `list_tenants(cookie)` | Visible mapped customers and current roles |
| `get_tenant(cookie, customer)` | Profile, manager-only members/invitations, current allowed sites |
| `manage_tenant(cookie, customer, TenantCommand)` | Profile, invite/resend/cancel, member role/remove |
| `list_invitations(cookie)` | Current user's pending invitations to active managed customers |
| `get_invitation(cookie, invitation)` | Recipient-only invitation, also supports standalone organizations |
| `respond_invitation(cookie, invitation, accept)` | Atomic verified recipient acceptance/rejection; canonical recipient pinned for native invitations |
| `account_sites(cookie)` | Enabled site grants and applicable inherited customer administration, live billing/CMS state |
| `start_preview(cookie, customer)`, `current_preview(cookie)`, `exit_preview(cookie)` | Read-only 15-minute persisted preview; invalid previews remain nonoperator until explicit exit |
| `dispatch_invitation_jobs(limit)` | Lease/revalidate encrypted outbox; bounded retries; injected `Delivery` sink |

`TenantCommand` and `AccessCommand` use tagged kebab-case `action` JSON. Tenant profile fields keep snake_case names. Profile input is boxed in Rust only. Roles are lowercase enums. `ChangeResult` contains `id`, `message`, and optional delivery state. `queued` is durable queuing, never a claim of sent mail. `invitation-queued-activation-unavailable` means invitation/grant persisted but the activation job could not be queued. Default delivery is unavailable; callers must install an explicit sink. Activation URLs use `IdentityService`'s separate durable queue; schedule both dispatchers.

Customer owner/admin invariants are serialized by locking the organization and retained database triggers. An admin cannot demote/remove another admin or owner; owner promotion requires an active verified fully set-up native user. Keep a real active owner/admin. Operator accounts cannot receive customer grants or be suspended through this API. Native invitations grant membership only on acceptance. Operator-created organizations do not acquire an artificial operator-owner membership.

## Reviewed offline authority activation

These APIs require a separate offline migration connection plus a current native MFA operator cookie. Do not expose them as public HTTP actions.

1. `inspect_identity_activation(offline, cookie, subject)` or `inspect_organization_activation(offline, cookie, organization)` returns exact external link tuples, credential-free memberships/grants, and an inventory SHA-256 digest.
2. Independently review identity ownership and intended access. Supply `ActivationReview { snapshot_sha256, evidence_ref, evidence_sha256 }`. The code does not claim to verify an external provider or email ownership.
3. `activate_local_identity(offline, cookie, subject, LocalAccountRole, review)` atomically checks the review, encrypts the previous account/factor/passkey/session/access snapshot, retires external links without reassignment, revokes sessions, removes projected memberships, disables grants, replaces the unknown local password, and requires fresh email/password/MFA setup. Existing bans remain. It queues bound account activation after commit and reports unavailable delivery honestly.
4. `activate_local_organization(offline, cookie, organization, Vec<ApprovedMember>, review)` applies only the explicit verified native member/role manifest, requires a native owner, retains existing member IDs where pairs match, archives the old state encrypted, disables previous site grants/cancels pending invitations, retires external links, and records exact bindings. Regrant site access separately. Unlisted members do not inherit access.

No account is matched, merged or rebound by email. Repeated activation and stale inventory digests fail. Existing last-owner guards may reject an unsafe transition; they are never disabled. Identity and organization transitions are separate: migrate projected owners safely by first placing a reviewed native owner, or complete the organization manifest first. Encrypted archives are retained evidence, not automatic rollback recipes.

## Offline migration and RLS

Register `migrations/0001_access_authority.sql` as native migration 0010. It adds four FORCE-RLS tables (`access_invitation_state`, `access_delivery_job`, `local_identity_activation`, `local_organization_activation`), narrow audit/preview helpers, exact-binding authority helpers, and guards retained native actor/organization/customer/membership policies. Runtime roles cannot modify activation ledgers or read their archives. Never run migrations at API startup.

The coordinated policy-generator changes are in `legacy/control-plane/src/db/database-rls-schema.ts` and the native fallback of `legacy/control-plane/src/db/clerk-rls.ts`; reinstalling policies preserves the guards and table classifications. Native scopes cannot reuse externally projected users/organizations. During overlap, the existing separately verified external-proof branches remain intact. OAuth additionally checks the same organization authority for bound clients and Console membership admission, including every token/refresh revalidation.

## Invitation onboarding

Ownership is given on purpose: operators and owners may invite a person as `owner`; administrators may not (`Forbidden`). The Console offers the owner role first while a customer has no owner and no pending owner invitation, which covers the step right after creating a customer. Owners and operators can make further members owners; the last-owner guard still applies. The invitation read returns `customerID`, and owners and admins who join a customer workspace confirm the prefilled customer details.

`get_tenant` returns every invitation's progress (invited, email delivered, account set up, joined) read on the auth connection after the manage check, plus the latest activity entries visible to the actor under row security.

Inviting an address without a local account sends a single activation mail whose link (`/reset-password?token=…&invitation=…`) creates the password, signs the person in and continues into the invitation; no separate invitation mail is queued for that person. Existing accounts keep the plain invitation mail. Operators receive the one-use link in `ChangeResult.link` to hand over directly; customer administrators never do.

## Verification and remaining integration

`cargo test -p webdock-access`, strict Clippy, and the retained-role integration test cover tenant separation, invitation recipient/acceptance, last-owner and role protections, real operator MFA, website ownership-transfer rejection, grants/revocation, preview expiry/exit, bans/session revocation, stale review rejection, encrypted authority transitions, and native OAuth/RLS denial before explicit organization activation. Test URLs are checked before connection and must name literal loopback `webdock_rls_test`; only created fixture rows are deleted. `tests/policy-generator.test.ts` checks policy reinstall classification and external-proof overlap.

The service is implemented; host HTTP routes, browser tenant/admin UI and offline review CLI remain parent integration work. Real Mail delivery is not enabled here. SSO client registration/signing stays in `webdock-oauth`'s offline APIs; this crate does not implement client registration UI, MCP connected-app management, external provider ownership verification, organization deletion/ownership transfer, or automatic archive rollback. Workers resuming retained externally projected jobs still need their domain-specific lease/proof rules; this service does not rewrite worker actors.
