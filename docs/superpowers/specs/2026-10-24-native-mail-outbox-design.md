# Managed Mail outbox and sending

Status: builder check passed; implementation is governed by the accompanying plan.
Base: `5fe5576fc624fa1d6c2509e94196707ed40a97a8`.

## Intent

Make sending through managed Mail durable, replay-safe and explicit about uncertain
delivery. Preserve the native Mail API and its existing authority, budget and receipt
semantics. Move private message custody and execution into the Mail runtime; stop
performing SMTP or provider delivery inside an ordinary action future.

The owner approved this milestone and a local SMTP-catcher proof. Webmail UI is a
separate next milestone. No production cutover, real mail delivery or live-data
migration is authorized by this implementation.

## Root decisions

- Keep one canonical Core control ledger. Native source authorization, allowance
  reservation, route commitment and job admission already commit atomically. Preserve
  that boundary rather than introduce a second independently authoritative queue.
- Managed Mail owns ingress orchestration, encrypted private payload custody,
  preparation, delivery, receipts and payload cleanup. Core retains the narrow
  transaction-bound admission/control/settlement adapter and identity authority.
- Include both plugin `mail.send` and the native `/v1/emails` API-key path in the
  replacement execution proof. A plugin-action-only queue is not completion.
- Keep legacy executables and retained records compatible until an explicit migration
  and consumer cutover. Do not remove old routes or workers merely because the new
  local path passes. No live worker is reconfigured in this milestone.
- Use the existing Rust message, Mail protocol and durable state contracts. No Redis,
  general-purpose autonomous jobs or new distributed queue framework.
- Retain the current safe retry rule: preparation leases can be reclaimed before
  handoff. Definitive rejection is terminal; a committed or uncertain handoff never
  becomes an automatic transport retry. This milestone adds no new provider retry
  policy, even for HTTP429 or SMTP4xx.

These are implementation choices made by root, not additional owner requirements.

## Preserved product boundaries

Follow `docs/architecture/mail-deployment-decision.md`. Installing Mail, configuring
domains and sending outgoing-only messages allocate no Stalwart engine. Explicit
incoming activation, including first mailbox creation, remains customer-owned.

Retained incoming reservations, including failed or suspended engines, never fall
back to direct sending. Legacy incoming records remain quarantined. Preserve the
existing incoming send denial while the separate incoming relay integration is absent.

Core stays the active Rust API; Console stays React/Vite and Bun remains the JavaScript
tooling. No provider credentials, Core database credentials or RLS signing secrets
are given to browsers, public gateways or a caller-selected worker.

## Canonical send lifecycle

The existing states and their meaning remain authoritative:

1. Admission derives source and ownership, validates current authority and routing,
   reserves recipient allowance and creates `staging` in one Core transaction.
2. The runtime stores immutable encrypted payload durably, then acknowledges its
   exact upload lease. Only the matching acknowledgement changes the job to `queued`.
3. An atomic claim creates a fresh private token and bounded lease. Expired preparation
   claims can return to `queued`; replaced tokens cannot begin or settle work.
4. Preparation checks payload length, digest, canonical envelope and configured route.
   Corrupt or unsupported preparation enters `preparation_review` without transport.
5. Current source, project/customer policy, installation, generation, grant,
   configuration, routing and budget are rechecked. `handoff` commits before any
   provider I/O is authorized. A lost begin acknowledgement does not cause a send.
6. One bounded transport attempt returns `accepted`, definitive `rejected` or `unknown`.
   Expired handoff becomes `unknown`, with bytes and allowance retained.
7. The exact original claim fences settlement. Identical receipts replay safely;
   contradictory receipts fail. A matching late definitive receipt may settle unknown
   without authorizing another external attempt.
8. Cancellation is possible only before handoff. Cleanup deletes only Core-selected
   terminal payloads after a durable receipt acknowledgement. Unknown/review data stays.

Queue acceptance is not provider acceptance. Keep the native HTTP202 job receipt and
originating-key status/cancel partition. Plugin action responses and Console messaging
must explicitly distinguish queued, accepted, rejected, cancelled, review and unknown.

## Idempotency and source authority

Core derives the stable action operation identity from persisted authenticated
admission. `InvocationClaims.id` changes on replay and must not identify a logical send.
The browser preserves one idempotency key across uncertain retries of the same send;
changed input with that key conflicts. No client supplies replacement tenant, actor,
runtime, route, budget or operation authority.

Native API-key sources preserve current hashed-key authentication, sender restriction,
revocation, verified native sender evidence and exact-key status/cancel checks. Plugin
TXT verification must not silently replace native SPF/DKIM/DMARC/provider evidence.

Plugin-originated sends require an explicit server-derived source variant and durable
provenance. Accepted Mail intent survives ordinary browser logout/session or invocation
expiry. Before handoff, the actor must still be an eligible operator or customer
owner/admin, the project/resource must remain active and owned, and the original grant,
feature/resource policy and relevant delegated-client write authority must still permit
the send. Capture only nonsecret authority lineage needed for that revalidation. Exiting
acting mode alone does not retract an already accepted customer Mail intent; the real
actor and original customer scope stay auditable. Explicit authority revocation or a
policy revision invalidates preparation. This is fixed Mail intent, not a general
autonomous invocation or generic browser-job token.

Authority acquisition precedes business/installation locks. Reuse existing lock order
and restricted pools; avoid awaiting another pool while holding a conflicting transaction.
Infrastructure lookup failures deny execution without fabricating confirmed revocation.

## Runtime and transport

Use pinned mTLS or private Unix transport and actual enrolled peer identity. Candidate
discovery never grants ingress, queue claims, handoff or customer effects. Revalidate
current runtime and exact send authority at each meaningful execution boundary.

The dispatcher is supervised by listener lifetime. SIGTERM and Ctrl-C stop new claims
immediately and bound any in-flight completion. Process cancellation or forced exit
after handoff retains uncertainty, not retryable work.

Preserve generic installer bootstrap: it generates the runtime certificate after the
reviewed recipe is admitted. Normal mTLS configuration may derive provider/worker IDs,
generation, exact raw manifest hash and runtime credential fingerprint when listener
and callback certificate DER agree. Core still verifies actual enrollment and every
signed lease. Unix, split-certificate or manual nondefault enrollment requires an explicit
matching provider binding; never derive its credential from UID. A static recipe must
not require a not-yet-generated certificate fingerprint or a Mail-specific installer branch.

Reuse Mail protocol commitments and one-use attempt issuance where applicable. Reuse
the reliable SMTP phase classification and relay provider behavior. An absent/malformed
final response after possible acceptance is unknown. Provider correlation IDs do not
constitute an idempotency guarantee.

New native control/ingress services use the existing authenticated channel and bounded
typed contracts. Keep them outside the generic action/job authority surface. No caller
selects an arbitrary endpoint, RLS purpose, transport credential or provider identity.

## Storage and lifecycle

Provider selection and executor assignment are distinct. The existing route's
`pluginID` identifies the provider, not the managed Mail process holding payloads.
Add a stable server-owned executor/payload-store assignment to canonical jobs. Managed
claims and cleanup select only that assignment; legacy filesystem workers cannot
claim PostgreSQL-backed payloads. Drain accounts for executor and provider obligations.
Payload-store identity must survive runtime generation changes.

Keep the existing payload encryption format and tenant/reference binding for retained
filesystem data. A managed PostgreSQL payload adapter may use private metadata storage
to avoid adding a runtime PVC, while preserving immutable writes, integrity, confidentiality,
staging and orphan/terminal cleanup semantics. Never reinterpret existing sealing formats.

Storage identity/password, sealing material and retained incoming data survive restart
and generation replacement. Do not regenerate missing masters during recovery.

Every acknowledged send has a real canonical Core row attributed to its runtime before
acceptance. Existing continuation accounting must see all staging/queued/leased/review/
handoff/unknown work. Draining rejects new admission, permits authorized accepted work
to finish and remains blocked by uncertainty. No queue-count heartbeat substitutes for
the installation fence. Preserve accepted Webmail payloads and separate Sent-filing hooks.

Do not modify native migrations0042–0044. Allocate any new forward migration only after
the interface freeze. If private payload schema or changed sending behavior makes the
old inline executable unsafe, publish logical schema2/readable[1,2]. The high-water
reservation must block old schema1-only rollback even after failed candidate startup.
Physical migration numbering and logical schema versions remain separate.

## Runtime extraction and native ingress

Create one Core-independent runtime library, depending on the existing message,
Mail-protocol and plugin-runtime packages, not Core identity/persistence/hosting.
Move canonical wire DTOs into Mail protocol and preserve compatibility re-exports.
Core keeps transaction-owning SQL and internal Webmail/SMTP-policy adapters; extract
the public send router rather than moving those internal routes wholesale.

Use an asynchronous private-payload interface with immutable put, scoped get and
scoped remove. Keep the legacy filesystem format, key marker and filesystem custody
adapter. Add the managed PostgreSQL adapter without queue/budget state in that table.

Compose the public native Axum router and generic PluginRuntime service on the same
pinned mTLS listener. Reuse installed Tonic Routes/Axum support; another payload wire
protocol is unnecessary if authenticated streaming HTTP preserves the contract.

The thin public gateway authenticates the native key before polling the body, selects
the managed endpoint from operator-owned enrollment and forwards a bounded stream over
pinned mTLS. It never proxies internal paths, follows redirects, retries a body or
accepts a client-selected backend. The plugin also checks the actual gateway peer.

Preserve native path/query/header rejection, source-scoped status/cancel, scheduling,
attachments/CC/BCC, body and concurrency limits, and allocated-ID error responses.
An authentication call that requires buffering the entire request is not equivalent.

Plugin `mail.send` keeps its bounded input but returns a truthful queued job receipt.
Its source bridge must preserve project relay and domain semantics explicitly. Native
key policy continues to use native evidence/customer provider binding. Neither path
may silently substitute the other's provider account or sender proof.

Preserve the existing display-name limits separately: plugin actions accept at most
80 Rust Unicode characters; native public Mail accepts at most80 UTF-16 units. The
shared action compiler/stored-message preparation uses an explicit narrow action path.
Only the trusted canonical action-source lease selects it; request JSON or HTTP headers
cannot widen native validation. Other native limits and canonical byte rules stay intact.

## Frozen control envelope and source forms

Add optional `NativeMailControl.Call` beside the existing Core plugin services. Its
request has `plugin_id` tag1, `generation` tag2 and `command_json` tag3; its response
has `result_json` tag1. Only actual authenticated current runtime `mail` is accepted.
Use at most163,840 command bytes and131,072 result bytes, decoding/encoding limits
164,864/132,096, a15-second client deadline without automatic retry, and64 bounded
service permits. Fixed safe status text never contains input or configuration.
Mail-send action input has an explicit128KiB byte budget, allowing its declared30,000
Unicode characters. The existing generic frame/document constants are32KiB/64KiB;
other actions/jobs retain their existing frame limit and configuration retains64KiB.
Apply the narrow exception only for authenticated plugin `mail`, action `mail.send`,
with no job marker, at original admission, payload verification and Invoke boundaries.
The control envelope also carries a token of at most16KiB and fixed metadata. Inputs
exceeding their own byte or character limits still fail independently.

Only the Mail runtime's Invoke service and the authenticated Core Mail-send client
request need224KiB decoding/encoding headroom for128KiB input,64KiB configuration,
16KiB token and framing. Other runtime instances retain128KiB transport decoding;
their application limits remain unchanged. No Mail-domain dependency or request flag
selects this compatibility exception. Test the complete real Invoke path, not only
the payload-authority callback.

The same per-action budget applies through public Console invocation, the declared
plugin HTTP route and the resolved MCP tool. A larger JSON/JSON-RPC envelope is allowed
only for the authenticated Mail-send selection; other routes, tools and commands keep
their existing limits. Do not stop at a direct Host or callback test while an earlier
public body parser still rejects the valid Mail input. Batch/unresolved tool payloads
must not inherit the exception.
Declare required service `{name:"native-mail-control",version:1}` in the signed
release. The configured Host advertises it only when the actual Core listener has
the backend. Keep existing product/action capability lists; add no general Mail
capability or autonomous-job grant. Missing service support fails closed.

The command body uses the strict existing `send.authenticate`, `send.preflight`,
`send.admit`, `send.status`, payload-commit/abort, claim, preparation-review, begin,
finish and cleanup commands. No caller-supplied worker purpose or caller identity.
Every command reloads actual runtime/source/lane state within its ledger transaction;
a preceding momentary `authorize_peer` call is not a fence.

Add `send.admitAction` with only `invocationToken`, original `input`, UUIDv4 `payloadRef`
and lowercase SHA-256 `privatePolicySha256`. Core verifies live original admission and
input hash, derives customer/project/resource/actor/operation, and recomputes canonical
request bytes, size and envelope from the owned domain's canonical resource name.
The shared message helper is used on both sides. Do not accept those derived fields
as additional public authority.

Use `source_id = action:<persisted64-hex-operation-id>`, idempotency key `mail.send`.
The server-built source descriptor retains the action/project/domain/actor/grant
provenance and private-policy digest. Add exactly this source shape to lease validation;
preserve existing native `key:<id>` and Webmail source shapes.

Add `send.actionStatus` with `invocationToken` and `{resource_id,id}` input. Authenticated
action name `mail.status` means read; `mail.cancel` means cancellation/write. Caller
JSON cannot select that authority. Status/cancel remain scoped to the owned domain.
For authenticated `mail.history`, the same command accepts only `{resource_id}` and
returns at most20 action-source summaries for that domain. It does not expose native
API-key records or payloads. Summary subject is derived at admission; envelope fields
already belong to the canonical job. Keep the bounded source context and reply limits.
The public action contracts are `mail.send` → `{id,state}`, `mail.status`/`mail.cancel`
→ `{id,state}`, and `mail.history` → `{items:[{id,state,from,to,subject,transport,at}]}`.
IDs are canonical decimal Core job strings. States retain the canonical outbox names;
only optional documented receipt metadata may accompany status, never private leases.

New bundled UI receives declared action names and only enables durable send/status/
history controls when that installed release declares them. It must not interpret an
older inline sender's `accepted:true` response as a queued receipt or offer unsafe
blind retries against that older runtime.

## Managed policy, private accounts and incoming transition

The managed provider branch requires external installation `mail`, exact verified
release/runtime/configuration/peer, and real current customer transport/provider
binding naming `mail`. Keep the legacy builtin-Mail-plus-external-provider branch
unchanged. Managed Mail does not fabricate an HTTP binding or owner/project.

Provider route fields retain their existing meanings. Provider binding revision is
distinct from action grant revision. Bindings and effective configuration are hashed
deterministically, and provider execution is an explicit legacy-HTTP or managed variant.
The one-use begin-attempt response carries a managed marker/source and Core-selected
transport mode/revisions, not client-supplied routing.

For native keys, the managed private delivery configuration must explicitly map the
Core-selected customer/account/mode/revisions to matching private transport material.
Missing or changed mapping fails closed; no arbitrary project-relay fallback. Treat
this operator-owned map as a reviewed cutover prerequisite, not automated provider
provisioning. Any mode/revision/account selection returned by Core must match it.

For plugin-action sources, the private project relay or configured platform transport
remains product-owned. Bind its effective policy/material to the admitted digest and
recheck it immediately before external I/O. Removal/rotation enters review or cancels
pre-handoff; it cannot silently choose a different account. Keep native sender evidence
and action-domain TXT evidence in their respective source branches.

Serialize managed outgoing admission/handoff and product incoming activation/first
mailbox reservation under the same customer fence. Check both retained customer and
legacy engine reservations before sending. Incoming activation must not race past an
accepted direct send with unresolved Core state. Use a narrow authenticated Core
barrier check if needed while holding the product customer fence; no approximate
heartbeat, unlocked count or automatic deletion resolves uncertainty.
The same barrier protects deletion of a sending domain while pending/review/unknown
work still needs it for access and reconciliation. A fixed `send.checkTransition`
command may derive supported Mail transition intent from the authenticated original
action; it cannot accept a caller-selected generic action or permission override.

The managed lane has a stable installation-scoped payload-store identity and executor
marker `managed-mail-v1`. Legacy workers exclude it; managed claim/reap/cleanup select
only that lane. Dispatch requires current generation, while terminal cleanup may cover
older generations of the same retained store. Admission requires enabled state;
accepted dispatch may run while draining; disabled permits only current-peer receipt
and cleanup bookkeeping, never a new handoff. Unknown/review blocks disable/update.

## Required proof

Use fresh owned TLS PostgreSQL, real Core authority/control and the actual Mail process,
with a loopback SMTP catcher. Test native API-key ingress and plugin action ingress.
HTTP/Host stand-ins may inject failure but cannot be the only authority evidence.

- Same logical key through fresh invocation IDs, lost replies and restart yields one
  job and one catcher message; altered input conflicts.
- Upload/claim/preparation recovery retains encrypted bytes and rotates leases safely.
- Cancellation, source revocation, incoming activation and disable race correctly with
  admission and handoff; wrong peer, tenant, project or payload cannot send.
- The catcher stores complete DATA then drops its final reply: unknown retains bytes
  and allowance and never resends after lease expiry, replay or restart.
- Lost begin/finish acknowledgements and stale workers never duplicate provider I/O.
- Draining sees accepted work and uncertainty; generation replacement cannot bypass it.
- Original keys, metadata/relay/incoming ciphertexts and retained identities remain.
- Outgoing-only use allocates no engine, mailbox, workload or PVC.
- Console says queued rather than sent until there is definitive acceptance; scope
  denial and uncertain retries preserve privacy and idempotency.

Preserve prior genuine Console/CE/PVC evidence as a separate baseline. A standalone
send proof does not assert a new Kubernetes rollout, public delivery, Webmail UI parity
or physical schema down-migration. Build the actual Mail OCI artifact if dependencies
or packaging change, and state precisely which paths were executed.

All fixtures record ownership before mutation, remove only exact owned resources and
private material, and leave retained dev/k3s state unchanged. Root owns actual execution,
whole-change review, full quality, signed integration and the final local-PC shutdown.
