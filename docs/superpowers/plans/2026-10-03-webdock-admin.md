# Webdock operator registry implementation plan

Goal: ship the first independent operator management application with customer/project CRUD and an inventory of already-provisioned CMS instances. CMS creation is never an implicit side effect of customer/project creation.

Architecture: `apps/admin` inside the public `spitzli/webdock` monorepo, with its own Vercel project; Next.js UI with a minimal Payload backend for authentication, access-controlled persistence and audit events. A dedicated `webdock_admin` schema/login in the existing Neon database contains only management records. The websites keep their independent deployments/admins and do not call this management application.

Spec: `docs/architecture/spitzli-control-plane.md`. User confirmed management scope and selected Snowflake on 2026-10-03. Execution: inline, using the existing authorization to implement, sign, push and deploy.

## Constraints

- English interface; system light/dark default, accessible responsive forms/navigation.
- Snowflake for platform-owned user/customer/project/CMS/audit IDs. PostgreSQL coordinates node 0 allocation using 41 timestamp bits, 10 node bits and 12 sequence bits; API and JS IDs remain decimal strings. Framework internal IDs are unchanged.
- Keep the current Payload/Next versions. Reuse native Payload authentication and the tested operator protection policy. No public signup or customer panel.
- Customer and project creation allocate no CMS infrastructure. Existing instances can be registered/edited as inventory; automated activation is a separate phase and must not be presented as a working action.
- No runtime database-owner/Vercel-master credentials. New management DB role cannot read website schemas. Instance records contain metadata/secret references only, never credentials.
- Prefer archive to deletion. Audit registry writes atomically with the originating transaction. Audit events cannot be changed through the API.

## Tasks

1. Bootstrap isolated new repository, dependencies and minimal Next/Payload routes. Add an allocator migration and test concurrent allocation/precision, sequence overflow and clock handling in disposable local PostgreSQL.
2. Add protected users, customers, projects, CMS instance inventory and audit collections. Verify anonymous/customer denial, immutable IDs, ownership relationships, unique CMS per project and no provisioning on normal CRUD. Preserve the real operator's password via a private controlled seed step.
3. Build the operator login and customer/project overview/detail/edit flows, CMS links and audit history. Read one local management database, never fan out to every website on navigation. Server actions repeat authorization; validation errors retain entered values.
4. Bootstrap existing customer/project/instance metadata from explicit known configuration. Record exact import source provider references, exclude secrets. Use a scoped production schema and manual migrations; previews must not automatically migrate production.
5. Validate builds/types/access, browser CRUD/logout/system theme/mobile layout using disposable records. Sign/push and deploy the management app, then verify production auth and the existing websites. Document features actually delivered and the remaining automatic provisioning step.

## Review focus

- Snowflake concurrency across serverless instances must not depend on an in-memory worker counter or JavaScript Number.
- Forged authenticated customer identities must not read management records or invoke server actions.
- Archived customers/projects must preserve references; an existing CMS cannot silently move to another project.
- Audit failures must roll back the business mutation; raw API writes must receive the same protection as UI actions.
- URLs displayed as links must be validated HTTPS origins/URLs without credentials or executable schemes; inventory saves never fetch arbitrary URLs.

## Steering and current boundary

The owner requested a public monorepo at `spitzli/webdock`; source now lives in `apps/web`, `apps/admin`, `apps/cms` and `packages/instance-kit`. Website/CMS deployments retain separate Vercel projects.

After reviewing the initial implementation, the owner requested discussion of the full auth architecture and proposed Neon Auth. Stop auth-dependent deployment, production operator seeding and new SSO implementation until that design is resolved. Continue the independent monorepo/source migration. The admin database schema exists, but the operator account has not been seeded and the admin app is not deployed.
