# Agent management protocol

This crate exports the generated `webdock.agent.v1.AgentControl` client/server contract from `proto/webdock/agent/v1/agent.proto`. It has no Docker, Kubernetes, CLI or execution dependencies. Core should depend on this crate and, when needed, `webdock-supervisor` with `default-features = false` for command/specification DTOs and fingerprints.

Core hosts Register, Heartbeat, Poll, Guard and Complete. The agent initiates every RPC over a reused authenticated HTTP/2 connection, locally through Unix sockets or remotely through required mTLS. There is no inbound agent management listener.

Every server handler must bind transport identity to the enrolled agent and installation: verify Unix peer credentials against enrollment, or the actual TLS peer certificate fingerprint, validity and revocation. Do not trust the self-reported Hello fingerprint. The fingerprint is empty for Unix transport and SHA-256 of the DER leaf certificate for mTLS. Protocol version and enrollment generation must match exactly.

`transport_identity(request, now)` extracts peer credentials and rechecks TLS certificate validity; it does not replace enrollment/revocation authorization.

Heartbeat may include a bounded retained-format `observation_json` from the actual adapter. Empty means connectivity-only and must not refresh old scheduling evidence. Sequence numbers are durably incremented before transmission. Accept strictly increasing values, allowing gaps after connection failures. Poll may return no command. Its timeout must fit the client's five-second RPC deadline.

Commands contain a bounded strict RuntimeSpec JSON document, fixed intent, operation/generation/lease, and an opaque authorization reference. Mutations may carry expected_runtime_id from the platform's last verified observation. Core must derive tenant/resource authority from the persisted operation, recheck its originating identity/grants and enabled/draining state, compare the request fingerprint and lease, and reject obsolete generations. Fingerprints come from `Command::fingerprint()` or `ReadCommand::fingerprint()` in the shared supervisor contract; a present mutation precondition is included.

Complete is at-least-once delivery. The daemon persists the exact receipt before sending and flushes it before Poll after reconnect; historical delivery does not need a new execution permit. Persist/deduplicate by operation, generation and request hash. A committed execution receipt may be replayed after reconnect without repeating its side effects. A failed or interrupted mutation reports needs_reconciliation, not a fabricated stopped/absent/success state. Logs/metrics are read permits with live authorization checks and 64-KiB result bounds. A mutation receipt's revision acknowledges the command intent; an independent Docker inspection reports its immutable deployment label revision.

## Incoming Mail

MailControl uses the same enrolled transport and cluster generation as AgentControl. Its strict
MailSpec binds the customer, immutable instance profile, operation/revision and material fingerprint.
Ephemeral MailMaterial contains either no allocation or the retained/bootstrap credentials; it must
not enter general journals or logs. MailDelivery contains only exact checkpoint/receipt custody.
Recorded accepted checkpoint metadata permits safe daemon restart without replacing credentials.
Expired or superseded callbacks cannot renew execution authority or manufacture readiness.
