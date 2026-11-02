# Webdock Mail gateway operations

This runbook covers the additive `webdock_auth.mail_*` product. The deployed native
incoming service and its retained storage remain governed by
[the Mail deployment decision](mail-deployment-decision.md). Incoming-enabled
customers currently fail closed with `native_submission_required` in the new
product dispatcher; their existing native sending path remains the supported path.
Do not treat the gateway implementation as an incoming-mail cutover.

## Controls and diagnosis

Platform administration exposes three independent, default-off settings:
`mailGatewayEnabled` gates both managed and BYOK gateway mutations;
`mailDispatchEnabled` gates new handoffs; `mailWebhooksEnabled` gates customer
webhook attempts. Existing `mailEnabled` still controls managed Mail, and existing
BYOK entitlement remains required. Authentication emails use their existing path.
These controls do not claim to stop clients sending directly by SMTP.

A connection can also be paused using its current revision. Pausing retains
history and queued work. Deleted/replaced connections terminate unstarted work;
uncertain handoffs remain reconciliation cases. Removing a customer team member
revokes their current access, but tenant API keys and schedules belong to the
customer and require explicit revocation/cancellation.

The Mail settings page shows credential/callback readiness, last successful job
ticks, blocked schedules, unknown outcomes and pending webhook deliveries. Shared
callback-source backlog and quarantine counts are operator-only. Investigate
missing dispatch ticks after three minutes, due work after five minutes, callback
backlog after two minutes and unknown handoffs after five minutes. An idle sender
does not need periodic provider callbacks.

For an authenticated operator session, save the native Auth Cookie header in a
private file (mode 0600, no shell history). From `apps/auth` with Bun:

```sh
bun --env-file=/private/auth-runtime.env scripts/mail-operator.ts health CUSTOMER CONNECTION /private/operator-cookie
bun --env-file=/private/auth-runtime.env scripts/mail-operator.ts reconcile CUSTOMER CONNECTION /private/operator-cookie MESSAGE_ID
bun --env-file=/private/auth-runtime.env scripts/mail-operator.ts pause CUSTOMER CONNECTION /private/operator-cookie CURRENT_REVISION
```

`resume` has the same arguments as `pause`. Reports contain operational metadata,
not message bodies or credentials. Health and reconciliation work with the gateway
switch off. Reconciliation reads only the exact historical provider account/MID
and an actual handed-off recipient. Without proven identity, wait for a scoped
callback and investigate the provider account. There is no raw resend command.

## Job deployment

The four HTTPS handlers are `/api/cron/mail/dispatch`, `/events`, `/webhooks`, and
`/maintenance`. They require the production Auth origin, `NODE_ENV=production`,
`VERCEL_ENV=production`, and a constant-time validated `Bearer CRON_SECRET`.
Copying a secret into Preview cannot enable jobs.

[The Kubernetes template](../../infra/mail/cronjobs.yaml) targets the existing
Contabo control cluster and is **suspended**. It is not installed by a database
migration or an application build. Confirm the existing control namespace before
applying it. Supply a strong random `CRON_SECRET` through the existing secret
manager, identically to production Auth and the namespace's `webdock-mail-cron`
Secret. The YAML never contains that secret. Do not reuse a tenant mailbox namespace.

Use exactly one scheduler. The current Auth Vercel configuration retains its
existing storage cron and has no Mail crons. If an operator instead confirms a
Vercel plan supporting minute ticks, use those same handlers there and leave the
Kubernetes jobs suspended. Do not activate both. Vercel CLI 63.1.2 was installed
with `bun add -g vercel`; read-only authentication confirmed the Spitzli team. This
worktree is not linked to a deployment project. Explicitly verify project and
environment before pulling configuration or deploying.

The template runs dispatch/events/webhooks each minute and maintenance at minute
17. It uses `concurrencyPolicy: Forbid`, no service-account token, a pinned curl
image, non-root execution and a read-only filesystem. Application leases remain
necessary across overlapping/manual ticks. Jobs have an absolute 45-second SQL
and network budget; two seconds are reserved for reporting. Expired SQL connections
are destroyed, rolling back unfinished transactions. Each dispatch tick visits at
most 20 tenants and five units per tenant using a persisted round-robin cursor.
Webhook concurrency is at most four. This is a capacity limit, not a measured SLA.

## Release sequence

1. Take and restore-test database backups. Keep `BETTER_AUTH_SECRET` for Mail
   product credentials/attachments and the separate native `MAIL_ENCRYPTION_KEY`
   recoverable through the existing secret manager. Never rotate either casually.
2. Apply additive Mail migrations using the explicit database owner URL and runtime
   role. Apply tenant-owner guards through the offline tenant migration. The
   runtime receives DML only; builds/request handlers do not migrate schemas.
3. Deploy Auth and Admin with all new Mail flags off. Do not roll back later to an
   application version that ignores the new owner boundaries.
4. Run the metadata backfill in dry-run mode, review its account/revision mapping,
   then apply while the gateway is off. See [migration](mail-migration.md).
5. Prepare the explicitly authorized pilot connection, verify its real account ID,
   sender ownership, SPF/DKIM and callback routing. Inventory the existing provider
   callback consumer first; do not replace a shared parent callback without a
   durable fan-out/cutover plan. Configure callbacks only for a proven source.
6. Confirm one scheduler, enable it, verify successful job reports, then enable the
   pilot gateway/dispatch flags. Customer webhook delivery is a separate switch.
7. Verify browser → API → database → provider → scoped callback → signed customer
   webhook. Observe the pilot for 24 hours before broader tenant enablement.

This sequence is a release procedure, not evidence that production has been
migrated or released. The independent managed-provider/callback proof, incoming
integration, load/restore evidence and 24-hour pilot are outstanding release gates.

## Retention and rollback

Bodies remain for 30 days after terminal acceptance/failure/cancellation; active
schedules retain required payloads. Attachments are erased at definitive completion.
Unknown payloads expire after seven days, while outcome, dedupe and unresolved
quota reservations remain. Metadata/events expire after 90 days, redacted request
logs and finished delivery attempts after 30, processed/quarantined callbacks after
seven. Cleanup is bounded and preserves active dependencies and leases. Old
terminal reservations can be repaired without releasing unknown outcomes.

Rollback disables gateway admission and dispatch, then pauses the chosen scheduler
and customer webhook delivery. Keep provider callbacks ingesting durably if the
handler contract remains compatible. Retain all rows, keys, encrypted secrets,
volumes and native instance configuration. Never down-migrate accepted work,
truncate queues or turn unknown states into pending sends. Re-enable only after
account identity, current leases, source consumers and outstanding outcomes have
been checked. A callback consumer cutover needs its own tested reverse route.
