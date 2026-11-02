# Native Webdock sending service

This service handles native API and Webmail sending, behind `WEBDOCK_NATIVE_MAIL_SENDING_ENABLED`.
It is not a production release of the complete Mail product. Incoming-enabled customers
remain blocked until their Stalwart submission policy/relay configuration is ready.

Run the offline Auth native-Mail migration, configure Auth's `MAIL_API_ORIGIN`, and provide
the API process with `WEBDOCK_AUTH_ORIGIN`, a separate `MAIL_SEND_WORKER_SECRET` (distinct from the browser gateway secret), the existing
`MAIL_ENCRYPTION_KEY`, and a dedicated private retained `MAIL_PAYLOAD_DIRECTORY`. Start
with `npm run api --workspace=@webdock/mail`. The same process runs the durable worker.
Do not use ephemeral storage or the public CMS blobstore. `Dockerfile.api` builds from
the repository root. Keep the feature flag false until infrastructure and accounts are ready.

An authorized administrator connects managed sending (after its existing sending account
and domains are verified), or selects an entitled existing BYOK connection, in Studio's
Mail → Webdock Mail sending. Webdock keys are shown once; provider credentials stay private.
A limited key stays limited if its domain is later removed.

`POST /v1/emails` uses `Authorization: Bearer wd_mail_…` and accepts JSON:

```json
{
  "from": "Support <support@example.com>",
  "to": ["recipient@example.net"],
  "subject": "Your request",
  "text": "We received your request."
}
```

Use a stable `Idempotency-Key` header when retrying the same request. An omitted key
creates a new submission; it cannot protect a client that retries after a lost response.
CC/BCC accept an address or address array. Optional `html`, `scheduled_at` (UTC ISO, up to
30 days), and `attachments` are supported. Attachments use `filename`, base64 `content`,
optional `content_type` and `content_id`; remote paths/URLs are rejected. Limits: 100 total
recipients, 10 attachments/10 MiB decoded attachment data, 16 MiB JSON request, and a
bounded final MIME message. At least one To recipient is currently required.

A 202 response has string `id` and queue `state` and is returned only after durable payload storage. Concurrent duplicates still uploading return 409 with their existing ID. `GET /v1/emails/:id` reads that key's
status; `DELETE /v1/emails/:id` cancels queued/preparing jobs. `accepted` means the next
transport accepted the message, not final delivery. `unknown` must not be retried as a
new message without reconciliation. No automatic resubmission occurs after handoff.

Per customer, the queue admits at most 1,000 unresolved jobs / 256 MiB of plaintext payloads.
A short staging lease reserves metadata and budget before any file is created; an abandoned upload expires without sending. Successful/definitely rejected/cancelled jobs are cleaned idempotently, with acknowledgement after unlink and directory sync. Unknown handoffs and preparation failures retain their payload. The disk free-space guard stops new writes before exhausting storage. Receipt metadata and idempotency records
remain in PostgreSQL.

Local verification includes the real HTTP API, native key authorization, PostgreSQL,
encrypted files, worker and relay adapter with a fixture HTTP response. No real external
messages are sent by these tests. Webmail also uses this durable outbox: an explicitly
authorized draft is exported to MIME, queued with its recipient budget, submitted through
the pinned Stalwart service account, and filed to Sent separately. A failed Sent update
can be repaired without sending again. Logout does not retract an admitted send, but
a revoked grant or disabled user prevents a queued handoff. Local HTTP/DB tests use fake
JMAP/SMTP peers; a separate isolated real CE test proves draft export, SMTP submission
and Sent filing. Real public delivery remains a required integration check.

Run the browser gateway and API as separate containers in the same pod. The gateway
uses `MAIL_GATEWAY_SECRET` and reaches `/internal/webmail` over loopback; the worker
uses the distinct `MAIL_SEND_WORKER_SECRET`. Never expose `/internal` through ingress.
The instance DATA hook uses its own escrowed per-instance token, bound submission proof
and queue ID. Hosting-agent configuration version 3 verifies relay credentials, the
persistent TLS pin, policy rejection and egress isolation before declaring readiness.
Managed private provider credentials need SEND_API and SEND_SMTP, never APIS.

Accepted Webmail payloads remain retained until Sent filing completes. Unknown handoffs
and unreadable payloads require reconciliation; they must not be automatically resent.
