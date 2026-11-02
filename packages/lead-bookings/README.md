# Lead bookings

Native Payload 4 plugin for independent demonstration sites. Add `leadBookings()` to `buildConfig({plugins:[]})`. It adds the protected `demo-requests` collection. The public endpoint must validate same-origin, bounded JSON and `validateRequest` before `saveRequest`; see `webdock-demos/template/src/app/api/demo/requests/route.ts`.

No email, payment or availability provider is involved. Anonymous native REST access and native creates are denied. CMS staff can inspect; editors/admins/operators can mark reviewed/archived. Submission fields are immutable. Database uniqueness handles idempotency races.

Independent sites vendor `src/` under `src/webdock-lead-bookings/`. From Webdock root run `bun packages/lead-bookings/sync.mjs /absolute/site/root`, then typecheck/test the site. `bun run --filter @webdock/lead-bookings test` checks validation, native access, HTTP boundaries and retry/race handling. Deployments still require database migration and HTTP persistence smoke checks.
