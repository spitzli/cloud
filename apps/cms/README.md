# Webdock CMS operations

The shared multi-tenant CMS is retired. The hosted app only links to the independent website admins and returns HTTP 410 for the old CMS API. It does not initialize Payload or connect to a database at request time.

| Website | Admin | Database schema |
| --- | --- | --- |
| Webdock | https://webdock.dev/admin | `webdock` |
| Spitzli Development | https://spitzli.vercel.app/admin | `spitzli` |
| Stall Eichenbruch | https://www.stall-eichenbruch.de/admin | `stall` |

`spitzli.dev` is not yet routed to the Vercel application. Existing account passwords are retained; sessions are independent. Each app has a scoped database login, unique Payload secret and protected operator account.

## Operations retained here

The old Payload configuration/migrations and migration tools remain for controlled offline verification and recovery. They are not exposed through this app's routes. Never run old migrations against an instance schema.

- `scripts/provision-instance-schemas.mjs`: restricted PostgreSQL schemas/logins.
- `scripts/export-instances.ts`: current central content, drafts, versions and account hashes to ignored private exports.
- `scripts/copy-instance-media.mjs`: copies public media into instance prefixes with an ignored resume ledger.
- `scripts/import-instance-users.mjs`: preserves account credentials during the one-time migration.
- `scripts/verify-instance-source.mjs`: read-only source parity and real cross-schema SQL-denial checks.
- `scripts/sync-instance-env.mjs`: installs scoped instance credentials after disconnecting old automatic database links.
- `packages/instance-kit`: canonical operator policy vendored into each independent app.

Private backups, exports and environment files remain ignored. The old database `public` schema and original media are retained for rollback. Old Neon resources are disconnected from the application projects, not deleted. Restore only after reconciling content edited since cutover.

## Development and checks

Use Node 24 and `npm ci`. `npm run dev`, `npm run build` and `npm run start` serve the retirement notice. `npm run lint` checks the repository; historical policy tests use the private `.env.local`. The old HTTP integration test is opt-in with `LEGACY_CMS_INTEGRATION_TEST=1` and requires an isolated checkout of the former CMS, not the retired production service. The retirement route check is `node --test tests/retired.test.mjs` against a local server on port 3119. Offline Payload commands still need the old central database credentials, kept outside the hosted application's route code.

## Future management panel

See [the control-plane design](docs/architecture/spitzli-control-plane.md) and [migration plan](docs/superpowers/plans/2026-10-03-isolated-instances.md). Customers/projects may exist without Payload; activation defaults off. A customer panel, dynamic collection designer and platform-wide page builder are intentionally not built in this phase. Platform-owned IDs are awaiting the ULID/Snowflake choice. Per-site Plausible tracking remains on the TODO list.

The [retired architecture](docs/architecture/retired-unified-cms.md) is historical reference only.
