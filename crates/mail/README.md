# Native Mail runtime

This private crate implements native Mail API, gateway and worker entrypoints. Production cutover and the complete Mail product parity gate remain separate work.

Implemented and tested:

- Strict send requests, canonical envelope recipients, private BCC, bounded inline attachments and MIME compilation through [mail-builder](https://docs.rs/mail-builder/1.0.0/mail_builder/). No filesystem or URL attachment source is accepted.
- Exact retained header sanitation, message digest and submission HMAC. Node-produced fixtures verify compatibility; existing encrypted payloads are opened without rewriting them.
- Mail's original AES-256-GCM envelope using RustCrypto. Mail uses a canonical base64 32-byte key and `webdock-mail:v1:` associated-data prefix, distinct from Hosting/database encryption. Preserve the existing Mail key.
- A private retained payload store, immutable atomic creation, fsync, owned directory descriptors, no-follow reads, size/free-space checks and key/tenant/reference binding. Use a bounded blocking worker for its synchronous filesystem operations.
- Transactional outbox claim, staging, review, cancellation and idempotent definitive receipts over the retained tables. Preparation can be reclaimed; a network handoff cannot be retried. Unknown acceptance keeps the job and budget.
- Recipient budgets shared with retained product usage, tenant and incoming-service state checks, concurrent reservations, idempotent settlement and month transfer. Incoming suspension cannot silently switch a customer to direct sending.
- A single SMTP attempt to the assigned private engine using [mail-send](https://docs.rs/mail-send/0.6.2/mail_send/), `smtp-proto` and Rustls. Verify the controller's exact current certificate before authentication, require advertised UTF-8/8-bit capability, bound the attempt and distinguish definite rejection from lost final acceptance. Loopback SMTP construction exists only under the explicit `test-support` feature.
- Account-bound JMAP reads, drafts, attachments, replacement state fences and independent Sent filing. Management credentials remain separate. Bounded Reqwest transport refuses redirects and retries. Assigned private engine hostnames retain their existing protocol contract.
- Native Webmail launch/exchange/inspection and write journaling with explicit mailbox grants, current native session/first-party browser authority, tenant/service state, grant revisions and coordinated IDs. Operator role alone does not grant mailbox content access. Old external proof rows remain retained but require reopening through native authentication.
- Browser gateway routes with exact HTTPS Origin admission, private path-scoped cookies, session/request/upload limits, durable write admission, post-response authority rechecks and secret-free public receipts. The `webdock-mail-gateway` executable includes a native healthcheck; its process startup, origin rejection and shutdown were exercised locally.

The outbox/budget functions require an already authorized, signed-scope transaction; they are internal primitives, not standalone public authorization. The caller must authorize source, sender, feature state and transport, reserve budget and transition handoff atomically, then commit before network I/O. New IDs must come from the coordinated native allocator; no new fallback SQL allocation is introduced.

Native API keys and sending admission use current source/domain authority and the same transaction as budget reservation. Admission stores an exact provider/runtime/configuration binding; begin checks it again before committing handoff. Core signs a sixty-second MIME/metadata lease and persists it in `provider_lease`. Authenticated plugin callbacks check current policy and atomically consume that lease before returning only the customer `send` HTTP binding. Unknown acceptance and lost acknowledgements never authorize a second external attempt. The provider stays in the independent `plugins/mail-relay` package.

The loopback submission API supports public sends, the private SMTP data hook and confidential Webmail export/stage/commit/status/cancel/filing. Accepted Webmail intent survives logout but loses authority on a current user ban, membership/grant revocation or route change. Sent filing is separate and retains the MIME until acknowledged.

Outstanding parity includes provider administration and reviewed legacy transport adoption, managed incoming provisioning/reconciliation, product operations, release packaging and complete frontend/cutover acceptance. No production delivery or provisioning is performed by the tests. Incoming Mail retains the per-customer Stalwart Community Edition decision; outgoing-only must not provision an engine.

`incoming::NativeIncoming` implements the retained incoming-service and mailbox administration
contracts through native session authority and restricted PostgreSQL roles. Service/directory reads
allocate nothing. Explicit operator activation reserves the existing per-customer instance key and
verified cluster capacity, then records a pending operation; it does not report runtime readiness.
Outgoing reservations or uncertain sends block the first transition to incoming Mail. Suspension
retains instance credentials, mailboxes and storage reservations. Only a previously verified instance
awaiting review can be explicitly reconciled, and a still-running operation prevents that retry.
Customer archival does not prevent an operator from suspending retained infrastructure.

Domain attachment requires current sender-domain ownership. Mailbox creation, local identity grants
and revocation update configuration revisions and durable operations atomically with their audit
events. New IDs use the coordinated allocator. Quotas and per-customer serialization prevent concurrent
over-allocation. Local operator roles never substitute for an applied mailbox grant; pending grants
and legacy external identity grants do not open content. Draining allows reads, suspension,
reconciliation and revocation, while rejecting new activation, domains, mailboxes and grants.
The same revocation rule applies to native API keys without relaxing tenant management permissions.

Core exposes these typed contracts through the existing `/api/console/mail` and `/v1/mail` envelopes:
`getTenantMailService`, `manageTenantMailService`, `getTenantMailboxes` and
`manageTenantMailboxes`. This admission slice preserves desired and observed state separately.
The native lifecycle executor now has an explicitly owned local Docker acceptance path through the
enrolled agent. Managed Kubernetes lifecycle and configuration execution have separate owned-fixture acceptance; a
pending operation must not be treated as a successfully provisioned customer.

`tests/incoming.rs` exercises the domain and actual Core router on a fresh disposable database with
restricted roles. It covers current authorization, tenant denial, strict payloads, no implicit
allocation, pending-send fences, revision conflicts, quotas, duplicate requests, configuration
retries, immediate grant/key revocation, suspension retention and redacted public directories.
Run it with the explicitly verified local fixture environment:
`cargo test -p webdock-mail --test incoming --locked -- --ignored`.

Native migration 0023 records the original session/authority at admission and immutable dispatch,
material and callback fingerprints. The worker uses the same authenticated Hosting agent transaction
and enrolled generation. Initial credentials are encrypted with the retained Mail key before dispatch;
callbacks carry only private lease tokens, hashes and observed resource metadata. Exact accepted
callback replays acknowledge custody without extending leases or performing another mutation.
Older uncertain operations remain visible as `pendingReview`; explicit operator reconciliation can
clear that blocker without overwriting a newer desired revision.

`tests/incoming_agent.rs` exercises claim/redelivery, altered-lease denial, immutable provenance,
checkpoint/receipt replay, expired callback handling and revision-fenced reconciliation under real
restricted roles. Its runtime proofs are controlled test inputs. The separate
`agent-control/tests/native_mail.rs` proves the actual Core → enrolled daemon → restricted Community
Edition engine flow on an internal owned Docker network, including acknowledgement loss and
re-enrollment while preserving credentials, certificate and volume.

Agent administration takes the customer fence before service/operation/cluster locks on the same
worker transaction, using the existing signed agent RLS policy. Canonical identity/browser locks
remain in the authentication transaction; no second business connection waits for that customer.
Database failures propagate without acknowledging custody or retiring work. Guard checks the
database clock after blocking authorization/material reads. Checkpoint records its live disposition
with an inline deadline check. Successful completion checks lease and originating-auth deadlines
again in the final receipt write, after readiness/capacity projections and their possible lock waits.
Expiry rolls the whole transaction back; a later exact delivery can retain stale custody without
applying readiness. Configuration checkpoints likewise recheck after provider-reference writes. Running success requires an observed submission TLS pin; suspension may omit it.

An unverified initial Ensure whose lease expires remains `needs_review`; the existing reconciliation
contract requires a previously verified instance. It does not silently mark readiness or restart
bootstrap. An explicit, separately reviewed inspect/reconcile workflow for that initial uncertain
state is still required. This also applies when the original admission session expires before first
dispatch. Historical operations without native provenance are not automatically adopted.

Run pure/filesystem/TLS contract checks with `cargo test -p webdock-mail --features test-support`. Ignored PostgreSQL tests require explicitly owned local fixture credentials; the budget test creates and drops its own fresh database. The SMTP tests use a loopback capture server and never deliver mail externally.

`cargo test -p webdock-mail --features test-support --test engine -- --ignored` creates its own pinned Community Edition container and private volumes on an internal Docker network. It exercises real draft replacement, attachments, pinned TLS SMTP delivery between two fixture accounts, JMAP reads/Sent filing and account/management denial, then removes only its own resources. Loopback TCP forwarding reaches the internal network without enabling internet egress. The upstream engine image retains its declared `NET_BIND_SERVICE` file capability in this test; this is not a claim about the separate restricted production wrapper or runtime provisioning parity.

Gateway configuration retains `WEBDOCK_AUTH_ORIGIN`, `MAIL_GATEWAY_SECRET`, `MAIL_BROWSER_ORIGINS`, `MAIL_SEND_INTERNAL_ORIGIN`, `HOST` and `PORT`. The optional `WEBDOCK_MAIL_CONTROL_CA_FILE` supplies a bounded public certificate bundle; hostname verification remains enabled. Production builds omit `test-support`.


The `webdock-mail-api` executable accepts `--config /absolute/private.json`. Its owned mode-0600 configuration contains `port`, `controlOrigin`, `controlSecret`, optional `controlAuthority`, `payloadDirectory`, `payloadKey` and optional `webmailSecret`. Omitting the Webmail secret disables the internal Webmail route. It binds IPv4 loopback; the public ingress must expose only `/v1/` and health endpoints. `controlSecret` is Core's separate sending-worker credential. The internal Webmail secret belongs to the browser gateway, and each SMTP hook has its own tenant token.

The `webdock-mail-worker` executable takes `/absolute/private.json` as its only argument, with the same private-file ownership rules. Configuration contains `controlOrigin`, `controlSecret`, optional `controlAuthority`, `payloadDirectory`, `payloadKey`, `issuerPublicKeyHex` and `providers`, an array of `{binding,channel}` entries. Up to 32 distinct `(pluginID,generation)` routes may be configured. Each binding tuple is `{pluginID,generation,descriptorSha256,workerID,workerGeneration,workerCredentialSha256}`. A channel is `{kind:"unix",path}` or `{kind:"mutualTls",origin,domain,certificate,privateKey,authority,certificateSha256}`. The worker selects an operator-configured route before handoff; missing, stale or unconfigured routes enter preparation review with their payload and budget retained. An empty provider array supports only the native private SMTP path. Neither command accepts a provider endpoint or credential from message data. The worker finishes an in-progress attempt before shutdown and does not retry its transport; a later queue poll may recover only pre-handoff work.

Focused acceptance: `tests/sending.rs` creates and drops its own database, bootstraps the complete schema, uses restricted runtime roles and the production Core policy adapter, and exercises a real authenticated callback socket. `tests/internal_send_api.rs` checks authentication before body polling, SMTP digest/proof forwarding and retained uncertain Webmail intent. These checks supplement the standalone provider's TLS HTTP/process tests; they do not by themselves establish a production rollout.


Cutover prerequisite: drain the retained sending worker, then review all remaining queued jobs before enabling the native worker. A retained `{kind,mode,transportRevision}` route has no reviewed native plugin/runtime/binding fingerprint; the native `send.begin` moves it to `preparation_review`, preserving its payload and budget reservation, and issues no provider lease. Operators must explicitly migrate its binding through a reviewed procedure or cancel the unsent job. The runtime never infers adoption from matching provider credentials. Newly accepted native jobs may finish while Mail/provider installations are draining; draining rejects new admissions.


`tests/platform.rs` verifies the complete sending chain on a fresh restricted database. It uses the production Core router over HTTPS in the test harness and starts the native API, native worker and independent relay as separate processes. The relay reaches only an owned, CA-verified TLS provider fixture. Assertions cover accepted and uncertain receipts, exact recipient budget, idempotent API replay, worker restart without resend, key revocation before handoff, BCC privacy, secret redaction, and retained unknown payloads. Build the independent relay first with `cargo build --manifest-path plugins/mail-relay/Cargo.toml --features local-provider-test`, then run the ignored platform test with explicitly verified local fixture credentials. This does not exercise a deployed ingress, production TLS certificates, or public mail delivery.


`webdock-account-delivery /absolute/private.json [--once]` drains the existing encrypted Identity recovery and Access invitation outboxes. Configuration is owned mode-0600 JSON with `authDatabaseUrl`, `businessDatabaseUrl`, `authSecret`, `rlsSecret`, `accountOrigin`, and `mail:{origin,authority?,from:{email,name?},apiKey}`. Database connections use the shared canonical connection policy and must authenticate as `webdock_auth_bootstrap` and `webdock_auth_runtime`; remote databases require verified TLS. `authSecret` must match Core's current version 1 and retained unversioned encryption key. The Mail origin must be canonical HTTPS; `authority` is an optional owned mode-0600 CA bundle. `accountOrigin` uses the same HTTPS/explicit-loopback-HTTP policy as Identity. No SMTP or provider credentials are configured here, and the dispatcher never provisions a tenant, sender domain or provider binding.

Each job uses the stable `account:<id>` Mail idempotency key and a deterministic request body. Only a strict 202 receipt with a canonical Mail job ID and a durable queued/leased/handoff/unknown/accepted state acknowledges ownership transfer. Redirects, wrong certificates, oversized or malformed responses, staging, rejected, cancelled and preparation-review states fail closed. Existing outbox lease/backoff/attempt limits remain authoritative. The executable finishes current bounded work on SIGTERM/SIGINT; `--once` processes at most one job from each outbox and returns failure when delivery remains unavailable. Core's HTTP process only queues work; deployment must explicitly run this dispatcher after reviewing its native Mail API key and sender domain.

Recovery and activation wording matches the retained account mail. Native invitation/website-access jobs currently persist no organization/site label or role in their encrypted payload, so their messages use branded descriptions and their exact canonical invitation/sites links. No dispatch-time URL template is accepted. `tests/account_delivery.rs` covers all four kinds, exact retry bytes, accepted/unknown acknowledgements, TLS trust, redirects, secret redaction and strict responses. `tests/account_delivery_platform.rs` uses a fresh restricted database, actual native Core/Mail HTTPS routers and restarted dispatcher subprocesses: an intentionally lost queue acknowledgement yields one Mail job and reservation; activation consumes its token once; invitation and reset jobs reach the same native queue without duplicates. No public mail is sent by these fixtures.


## Native mailbox configuration

`WEBDOCK_NATIVE_MAIL_CONFIGURATION_ENABLED=true` is an explicit Core opt-in requiring native Mail administration, the agent and mailbox support. The agent separately requires `mailConfigurationEnabled: true` and an available configuration runtime. The optional `MailConfigurationControl` uses the same enrolled UDS/mTLS channel as lifecycle execution; disabled configuration returns no work, and older Core installations may return `Unimplemented` without disconnecting the agent.

Native0024 records original session/browser authority and the exact original member ID or operator authority. Removing and recreating a membership cannot revive its earlier admission. Member fencing precedes configuration locks on the same worker transaction. Historical jobs without provenance remain unclaimed and require an explicit authorized retry. Drain the retained configuration worker before enabling this native lane; do not run both controllers against the same queue.

Domain, Group mailbox and User identity changes use bounded snapshots, stable encrypted credentials and exact owned provider references. Resource handoff precedes each mutation; verified checkpoints retain references without marking configuration applied. Callbacks bind the normal runtime ID and exact request/lease. Lost acknowledgements replay exact custody; changed callbacks fail. Final readiness/applied writes and provider-reference checkpoints check the database clock after potentially blocked projections and roll back on expiry.

Revocation denies Webmail access immediately. Restriction jobs can only contract existing memberships/permissions; their mailbox references are read-only and cannot authorize mailbox/domain mutation checkpoints. Membership-detach triggers record immutable events without using the disabled legacy ID allocator; the Rust allocator supplies operation IDs at materialization. Contractions retain their original revisions independently of later full admissions. An unissued restriction whose grant was superseded is retired without touching the engine. Running or uncertain mutation handoffs still require review. Restriction completion never advances the full configuration's applied revision; `canRetryConfiguration` exposes authoritative eligibility for a fresh full retry.

The focused restricted-PostgreSQL tests cover original authority, two-grant detach with later full admission, concurrent event materialization, regrant before dispatch, nonoperator lock ordering, replacement membership, deadline rollback and exact callback replay. The separate real-engine and browser fixtures compose Core, the enrolled daemon and Community Edition. Native0024 remains draft until independent review and explicit offline upgrade acceptance; this does not authorize production rollout. SMTP relay routes, submission hooks, provider credentials and public delivery are outside this executor.
