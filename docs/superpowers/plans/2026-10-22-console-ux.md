# Console and account UX review

> Execute inline with superpowers:executing-plans. Preserve the existing design and runtime boundaries.

**Goal:** Make the existing Auth, Operator, workspace/customer and member-management UI coherent, recoverable and usable on small screens and with a keyboard.
**Architecture:** Keep the existing React/TanStack application and Rust contracts. Fix shared controls and navigation in place; use disposable browser fixtures for role/error cases and the owned native account fixture for security acceptance.
**Spec:** The owner's request in this conversation and the current native Console/account implementation. No new product capabilities or global user-administration API are implied.

## Constraints

- Preserve Inter, existing orange/light/dark tokens, routes and component ownership. No new design system or dependency.
- Only local disposable fixtures; no real mail, customer changes or operator-password changes. Never read `.dev-local/demo-owner.json`.
- Keep authentication, role and tenant boundaries server-owned. No secrets in screenshots, logs or committed files.
- Bun only. Commit only after `cargo fmt --all` and green `bun run quality`; preserve both local handovers.

## Task 1: account and workspace navigation

Files: `AccountUI.tsx`, `Shell.tsx`, `WorkspaceNav.tsx` and browser regressions under `apps/console/tests/`.

- [x] Reproduce account/invitation links opening member administration instead of workspace overview.
- [x] Send every known workspace handoff to `/workspaces/:id`; correct role text that promises unavailable Billing.
- [x] Reproduce customer switch/search offering no actual workspaces; show/filter the authorized workspace list, retaining operator search boundaries.
- [x] Make active members navigation and account skip-link focus consistent.

## Task 2: safe, recoverable forms and dialogs

Files: `CustomerMembers.tsx`, `AccountUI.tsx`, `common.tsx`, `ui.ts` and browser regressions.

- [x] Reproduce lost invitation input on a failed request; reset only after success.
- [x] Require explicit confirmation before removing a member; cancel and failed requests preserve access and context.
- [x] Handle clipboard refusal with selected manual-copy fallback and visible guidance.
- [x] Reproduce challenge mode changes during pending sign-in and inability to choose another account; disable conflicting actions and provide a return to sign-in.
- [x] Reproduce dialog padding clicks dismissing it and short-viewport overflow; fix the shared dialog.
- [x] Verify readable mobile form controls and long workspace/customer names without masking overflow.
- [x] Show actionable shared service/network errors.

## Task 3: full UI review and verification

- [x] Inspect Auth, account security/workspaces, operator lists/details/plans/clusters, customer overview/projects/domains/usage/tokens and members as operator, owner, admin and member.
- [x] Check 320/375/414/768/1280px, light/dark, keyboard focus, pending/failure/empty states and dialog behavior.
- [x] Run all Console tests and browser fixtures; run native account acceptance on an independent owned fixture where feasible without interrupting existing dev services.
- [x] Independent final review, address findings and run quality. Preserve the existing worktree and sign/push the existing branch under the owner's standing authorization.

Review focus: stale in-flight mutations; invitation data loss; accidental privilege/access changes; read-only roles calling operator endpoints; clipped long names or dialog controls; screen-reader focus and honest capability text.

## Findings and progress

Navigation now opens the selected workspace overview after account onboarding and filters actual
authorized workspaces in the customer switcher. Members navigation announces its current state;
Plans/Clusters breadcrumbs match their pages. Direct operator and API-token links show an access
message to unauthorized roles before mounting the editing UI. Backend authorization is unchanged.

Invitation fields survive failures and reset after success. Removing members requires confirmation,
clipboard refusal offers selected manual copying, and sign-in modes cannot conflict while a request
is pending. Verification can return to sign-in. Shared dialogs retain padding clicks, fit short
screens and wrap long member names; phone inputs use readable text. The switcher and account menu
now have explicit focus and keyboard behavior. Copy stays focused on available actions.

The browser matrix covers 23 route/profile combinations as operator, owner, administrator and member,
at 320/375/414/768/1280px, in light and dark. Screenshots use synthetic data and reduced motion for
settled captures. No layout overflow is hidden globally. Native account acceptance uses the independent
`account-browser-ui-ux.json` disposable profile; its real password/MFA/recovery/session/passkey/SSO
flow passed. Its outdated logout selector was corrected to operate the actual account menu.

The independent review found no Critical or Important issue and two Minor state-isolation issues.
Both were reproduced and fixed: member-management instances are keyed by customer, so cached
history navigation clears a stale removal confirmation; opening another member's confirmation
clears a prior mutation error. A successful one-time invitation link remains available. The React
check also verifies that the new member keys do not collide with the existing customer-form keys.

Final verification: `cargo fmt --all`, `bun run quality` and whitespace checks passed after the
review fixes. Quality includes Rust checks, Console lint/typechecking, unit tests, production build
and all fixture browser suites. The independent native account browser run also passed.

Limit: fixture screenshots and request-failure cases are UI checks, not proof of real mail delivery
or provider provisioning. Native passkey acceptance uses a virtual authenticator. Physical devices,
Safari and assistive-technology announcements were not independently verified. No production identity,
customer, password or authorization policy changed.
