# Webdock Console

Next.js frontend for the Webdock API. Business data and authorization live in
`apps/api`; Console holds no database credentials. Run from the monorepo root after
`npm ci`, using Node 24:

- `npm run dev -w @webdock/admin` starts on port 3120.
- `npm test` runs the isolated Console unit tests.
- `npm run check -w @webdock/admin`, `npm run lint -w @webdock/admin` and
  `npm run build:admin` check source and production output.

By default, existing deployments retain central Better Auth SSO. Setting
`WEBDOCK_CONSOLE_AUTH_PROVIDER=clerk` selects Clerk for Console login, account and
organization controls. API activation and the matching production identity
configuration must be prepared together. See [.env.example](.env.example) and the
[release/rollback guide](../../docs/architecture/clerk-console-release.md).

Clerk session tokens and the server-only delegation key go only to `/v1/console`.
The API verifies the live identity, mappings, customer state and membership before
issuing bounded database authority. CMS/OIDC/MCP keep their existing legacy
protocol; no Clerk session is converted into a Better Auth session.

Customer teams use native Clerk Admin/Member rules. Local user bans, customer
archival and explicit resource restrictions remain effective. Ordinary registry
CRUD does not provision infrastructure. Incoming Mail is activated separately;
mailbox content still requires an explicit grant, including for the operator.

The existing `scripts/` provision/migration utilities are historical offline tools,
not the Console's deployment path. Current schema and identity migration are owned
by `apps/api/scripts/`; never run an old Payload migration against the live registry.
