# Managed Mail outbox

The local Core/process/SMTP acceptance passed all twelve dedicated cases. The
[dated verification record](../verification/2026-10-25-native-mail-outbox.json) separates
real authority and delivery paths, controlled fault injection and image packaging checks.
No production cutover or public delivery is implied.

## Ownership and prerequisites

Managed Mail 0.3.0 uses the existing atomic Core send ledger for source authorization,
recipient allowance, admission, leases and settlement. The Mail runtime holds encrypted
message bytes in its separate private PostgreSQL database and owns preparation,
execution and payload cleanup. There is no second queue or general-purpose job grant.

Configure the real NativeMailControl and MailAuthority services on Core's existing
authenticated plugin listener. The signed release requires native-mail-control/v1;
Host availability must correspond to the registered backend. Existing product
capabilities and customer/project authorization still apply.

The runtime's private `delivery` configuration supplies:

- `nativeLeaseIssuerPublicKeyHex`: the native Mail lease issuer's public key, which
  need not be the generic invocation issuer.
- `gatewayPeers`: exact native gateway certificate pins or local Unix UID peers,
  separate from Core probe/invocation peers.
- `nativeAccounts`: explicit customer/account/mode/revision mappings and their private
  transport material. Nullable account/revision fields must match Core exactly.
- Optional `providerBinding`: exact enrolled worker/manifest binding for Unix, manual
  or split-certificate profiles. Normal installer mTLS derives it only when listener
  and callback certificate DER agree, using the exact raw manifest hash and generation.

Private transport credentials never belong in a browser, public response or log.
Preserve metadata login/password, sealing material and the payload key sentinel on
every restart or replacement. The native filesystem payload format and plugin
credential sealing are distinct; neither is silently converted.

Allow at least 2GiB runtime memory for the preserved native body/concurrency limits;
size private PostgreSQL, WAL and retained uncertain payload capacity separately. All
Kubernetes resource budgets remain explicit operator-reviewed target configuration.

## Ingress and execution

`mail.send` retains its existing bounded fields and returns `{id,state}` after durable
queue acceptance. Console says queued rather than sent. `mail.status`, `mail.cancel`
and `mail.history` operate on an owned sending-domain resource; history contains at
most 20 action-source summaries, never native-key records or message bodies.

The native public API preserves `POST /v1/emails`, `GET /v1/emails/{id}` and
`DELETE /v1/emails/{id}`. HTTP 202 acknowledges the queue, not provider acceptance.
Native key authentication, sender evidence, scheduling, attachments, CC/BCC, originating-key
status/cancel partition and shared budget remain authoritative in Core.

The separate `webdock-mail-gateway` authenticates through existing private Core HTTPS
control before reading a public request body. It forwards a bounded stream once to
the configured runtime over pinned mTLS. The runtime verifies the actual gateway peer
and authenticates again. It exposes no internal control routes through that proxy.
There are no redirects, body retries, environment proxies or client-selected backends.

The runtime serves generic plugin RPC and native HTTP on its existing TLS listener.
Discovery remains available while the callback is unavailable or the generation is
still a candidate; no candidate may claim or send accepted work. SIGTERM and Ctrl-C
stop new claims and bound in-flight finalization and listener drain.

## States, retries and uncertain delivery

Staging persists custody before queued acknowledgement. Preparation claims have
private tokens and bounded leases. An expired preparation claim can be reclaimed;
handoff must commit before external transport is permitted. The native signed lease
and one-use MailAuthority attempt bind the actual worker, payload, envelope and route.

Accepted, rejected and cancelled are definitive outcomes. Preparation review retains
the message without sending. Handoff expiry or a possibly accepted message with no
definitive response becomes unknown. Unknown retains payload and reserved allowance,
and never automatically retries transport. A matching late definitive receipt may
settle the original attempt; it cannot authorize a new one.

Definitive SMTP/HTTP rejections are terminal in this release, including SMTP 4xx and
HTTP 429. This does not add a provider retry policy. A provider correlation ID is not
an idempotency guarantee. SMTP's internal submission correlation is not a Turbo MID.

Reuse the same logical idempotency key after a missing receipt. A 403 returned after
action execution is not proof that queue admission failed. Console preserves the
original snapshot/key through lost responses and background refresh errors, and
requires an explicit discard before starting a separate submission. Queue status is
read-only and never resends a message.

## Authority, incoming activation and replacement

Accepted action Mail intent survives ordinary logout or invocation expiry. Current
actor eligibility, project/resource ownership, grant/policy and delegated-client write
authority are rechecked before handoff. Native key sends retain their own key/domain
and provider evidence. Action-domain TXT verification never substitutes for native
sender SPF/DKIM/DMARC evidence.

Incoming activation and first mailbox creation share the customer's private fence
with outgoing admission and handoff checks. Canonical outstanding direct work blocks
unsafe activation or domain deletion. Any retained customer engine state or legacy
quarantine blocks direct fallback. Outgoing-only use creates no CE engine, mailbox,
workload or PVC.

Managed jobs have a stable installation-owned executor/store assignment. Legacy
filesystem workers cannot claim or clean their PostgreSQL payloads. Draining stops
new admissions and allows authorized accepted work to finish; review/unknown work
continues to block disable/update rather than being aged away.

Private migration 4 adds payload custody, not another queue. Logical schema 2/readable
[1,2] is separate from the physical journal. The monotonic schema watermark prevents
unsafe rollback to a schema 1-only inline sender even after a failed candidate start.

## Cutover and verification boundary

Cutover is explicit: review the real account/mode/revision map, gateway peer and pinned
endpoint, verify the managed lane, then route new traffic/execution to it. Keep old
records with their original custody and compatibility path until separately migrated.
Do not rotate keys, re-encrypt retained data, fabricate owners or redirect old consumers
as an incidental part of deployment.

The earlier Console/CE/PVC lifecycle acceptance is recorded in
[managed Mail installation](../verification/2026-10-24-managed-mail-installation.json).
It is a separate baseline, not evidence for this outbox's Core/SMTP chain. Historical
schema 1 cluster fixture scripts remain bounded to their original contract; current
outbox verification uses its dedicated owned native fixture and an independently
built Mail image. No cluster rollout follows automatically from those local checks.

Webmail UI, incoming relay/public reception, real provider delivery, backups and
production activation remain separate milestones. Unknown outcome requires definitive
evidence and the original fenced receipt; this release adds no generic force-resend
or force-reconciliation endpoint.
