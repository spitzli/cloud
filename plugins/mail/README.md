# Mail

![Mail icon](assets/icon.svg)

Manage project mail resources and inspect durable outgoing submissions from native Spitzli Cloud surfaces.

## What you can do

Mail domains, relay connections, explicitly enabled incoming mailboxes, retained suspension and API submission/outbox contracts.

## Setup and access

An operator installs the runtime, private delivery configuration and gateway bindings. Incoming Mail needs explicit customer activation; outgoing-only use does not provision Stalwart. Provider or domain setup is not automatic.

## Data and security

Core owns customer/project authority. Credentials are sealed, private payloads remain server-side and uncertain sends must be reconciled before resubmission. Incoming deactivation retains volumes, keys and mailbox records; deletion is separate.

## Status and limits

Beta. A queued receipt means custody, not delivery. Public reception, recipient-aware routing, Stalwart-to-Turbo integration and full native Webmail remain release gates. One isolated Stalwart Community Edition instance serves each explicitly enabled customer. Existing incoming reservations fail closed for unproven outgoing transitions.

Catalog publication does not activate the plugin, enable public signup or grant
access to any customer resource. A new presentation release must use a new version;
previously signed releases and their assets stay immutable.

## Gallery

Actual Mail mailbox panel; synthetic settings. No public reception, delivery or Webmail is demonstrated.

![Mailbox client settings with hidden password and synthetic addresses](screenshots/mailbox-settings.png)

## Technical reference

The retained implementation notes below include platform integration procedures.
Commands referring to Core, `tools/` or `apps/console` require the [platform source
checkout](https://github.com/spitzli/webdock); they are not standalone setup steps.

Outgoing mail as a Webdock plugin:

- **Sending domain:** a project adds a domain (one project per domain across the installation) and proves ownership with a `_webdock.<domain>` TXT record. The domain page lists the DNS records (ownership, SPF, DMARC) and the legacy send log; the durable outbox lists the latest20 action sends.
- **Own turboSMTP account (BYOK):** a project connects its own turboSMTP consumer key and secret. They are stored AES-256-GCM sealed, are never returned, and from then on every message of the project goes through that account (EU endpoint `api.eu.turbo-smtp.com`). Without one, messages use the platform transport from the private configuration.
- **Sending:** `mail.send` from a verified domain, at most 10 recipients, plain text. It is exposed in the Console, as `POST /v1/plugins/mail/projects/{project}/api/send` and as the MCP tool `send_email`.

Platform transport: `{"kind":"turbosmtp","consumerKey":…,"consumerSecret":…}` or, for local dev only, `{"kind":"smtp","host":"127.0.0.1","port":…}` pointing at a loopback catcher such as Mailpit. `trustUnverifiedDomains` (local dev, loopback catcher only) skips DNS ownership because `.test` names carry no DNS.

The signed managed installer supplies private `listenTls`, `coreTls`, `corePeers`,
`storageUrl`, `storageCa` and `secretKeyHex` runtime fields, using the same shapes as
the PostgreSQL plugin. Exactly one listener (`listen` or `listenTls`) and callback
(`core` or `coreTls`) are required. Existing absolute private Unix socket configs
remain supported. TLS callbacks verify the Core hostname, CA and exact certificate
pin; incoming probes require the configured Core peer. Remote metadata PostgreSQL
requires `storageCa` and verifies its actual hostname with no plaintext fallback.
Without a CA, `Store::connect` accepts only loopback endpoints, including driver host
overrides. Configuration and TLS files retain the shared owned-file checks.

Describe/Health listening starts even if the Core callback is initially offline.
The deferred callback transport retains CA/name/leaf-pin verification and checks
Unix socket ownership/permissions on every connection attempt. A listener-scoped
startup future waits for ordinary current-generation Core authority,
retrying denial/outage with backoff from 100ms to 5s. Only then does it call `resume`;
a failed resume requires fresh authorization before another attempt. The shutdown
signal cancels the wait, including an in-flight callback or resume future, before
asking the listener to drain active requests. Signal monitoring continues after
resume succeeds; listener exit also cancels startup.
Metadata migrations may run while the runtime is a candidate. Startup failure logs
only `mail_plugin_start_failed`. Update/rollback must retain the metadata database
and sealing key; runtime generation and certificate changes do not replace engine,
mailbox or relay credentials. Physical migrations001–003 stay unchanged; migration004 adds encrypted payload custody and its key sentinel only. Release0.3.0 uses logical schema2/readable[1,2]; schema1 rollback is unsafe once Core has reserved the high-water mark.

- **Incoming Mail:** explicitly creating the first mailbox or using `incoming.activate` on an owned domain reserves one Stalwart Community Edition engine for the **customer**, not the project. All of that customer's projects, domains and mailboxes reuse its sealed administrator credentials, workload unit `engine-c<customer>` and host `c<customer>.<domain>`. Customer identity comes from Core's signed `customerID`; request input cannot select a customer or engine address. Mailboxes, domains and relay resources keep project ownership, so sharing an engine does not grant access to another project's resource.
- **Retained lifecycle:** `incoming.deactivate` on an owned domain suspends incoming Mail for every project of the customer by scaling the engine to zero. Its workload, volume, credentials and mailbox records remain. The operation stays `suspending` until Core acknowledges the apply revision and observes the zero-replica workload; only then is it `suspended`. Activation and runtime restart reconcile the saved desired state; management failures never destroy or rebuild the volume. Mailbox deletion remains explicit; engine retirement/export/deletion is a separate operator operation, with no customer engine-delete action.
- **Legacy project engines:** the storage migration preserves old `engines` rows, sealed credentials and mailbox records. They have no trustworthy customer mapping in plugin storage, so incoming operations report **migration-required** and do not provision replacements or reconcile those engines. Because another project might belong to the same legacy customer, all incoming activation stays blocked until the operator explicitly migrates the retained records and storage. This change does not migrate deployed Spitzli assets. Keep the plugin encryption key and all retained volumes. The UI exposes the migration state; do not delete legacy records just to bypass it.

The customer engine uses `plugin.workloads` with `{customer, unit, objects}` and no project lifetime dependency. Core owns customer workload authorization, placement, limits and retention. The plugin refuses changed signed ownership for a project until its data is explicitly migrated. Existing deployments and outgoing-only projects allocate no engine merely by being loaded, adding a domain, connecting a relay or sending mail.

The engine's NetworkPolicy denies all egress. IMAP and SMTP submission use implicit TLS through Traefik's `imaps` and `submissions` entry points. With `issuer` set, cert-manager issues the customer hostname's certificate; otherwise the certificate is self-signed. Management uses the separately configured HTTP route guarded by the sealed engine administrator credential. Internal-only management and relay egress remain production gates.

**Delivery boundary:** public reception, recipient-aware SMTP routing and engine→Turbo relay integration are not enabled by this change. Outgoing-only API sends retain the configured platform/Turbo transport. Once a customer has an incoming reservation, API sends fail closed rather than falling back to direct Turbo, including while pending, failed or suspended. Legacy engine records also block the unproven outgoing transition. Suspension does not switch the customer to outgoing-only.

A domain is unique per project; across projects only one may verify it, and the first DNS proof wins. Deletes are idempotent.

Not yet here: DKIM signing for API sends, inbound mail from the internet or Webmail UI. Core remains the authority for durable send states and recipient allowance.

```sh
cargo test --manifest-path Cargo.toml
# Disposable loopback PostgreSQL for plugin storage; SMTP and turboSMTP are in-test stand-ins.
# Mailboxes test two isolated customer engines, cross-project reuse, retained suspend/resume and restart reconciliation
# against real Stalwart CE on local dev k3s, applied by an in-test stand-in for Core:
WEBDOCK_MAIL_PLUGIN_TEST_ADMIN_URL=postgres://… WEBDOCK_WORKLOAD_TEST_KUBECONFIG=$PWD/.dev-local/k3s/kubeconfig.yaml \
  cargo test --manifest-path Cargo.toml -- --include-ignored
```


## Durable sending in release0.3.0

`mail.send` preserves its original input and returns `{id,state}` after Core staging,
immutable private payload custody and Core payload commit. Queued means custody, not
provider acceptance. `mail.status` and `mail.cancel` take `{resource_id,id}`;
`mail.history` takes `{resource_id}` and returns at most20 owned-domain action rows:
`{items:[{id,state,from,to,subject,transport,at}]}`. API routes `/status`, `/cancel`,
`/history` and matching MCP tools expose those declared actions. Domain `recent`
continues to mean legacy history. A missing private delivery configuration disables
only the durable send/outbox capability; it never falls back to inline delivery.

Core is the only queue and budget ledger. Private PostgreSQL stores immutable bytes,
length/hash and a key sentinel with no domain/customer cascade foreign keys. Retain
`secretKeyHex`: a purpose-separated SHA256-derived payload key uses the native v1
cipher/AAD `webdock-mail:v1:payload:<customer>:<reference>`. This does not reinterpret
or rotate existing relay, mailbox or engine credential ciphertexts. A wrong retained
key or corrupt/mismatched payload fails closed. Pending/unknown payloads require Core
reconciliation; deleting a domain does not delete their bytes.

The listener owns one supervised dispatcher. Candidates can expose generic discovery
but cannot claim, resume incoming effects or send. Every new claim requires ordinary
current authority; Core fences each job and permits accepted work while draining.
Handoff acknowledgement precedes the separately signed, one-use
`MailAuthority.BeginAttempt`. Lost/malformed acknowledgements and lost final SMTP
acceptance remain unknown; rejected is terminal, including SMTP4xx/HTTP429. There is
no provider retry. SMTP accepted receipts use `internal_submission_<Core job ID>` as
internal correlation only; this is not a turboSMTP MID. Unknown bytes remain retained.
SIGTERM/Ctrl-C stops new claims immediately and allows at most50seconds for an active
attempt and receipt/listener drain. Forced termination after handoff leaves Core's
uncertain obligation for reconciliation.

A dedicated private PostgreSQL transaction/connection customer fence serializes send
admission/handoff checks with incoming reservation and sending-domain deletion across
processes. It never runs transactions on the shared metadata Client. Fence capacity4,
acquisition/lock deadline5seconds and idle transaction ceiling60seconds bound waits.
The authenticated Core transition barrier rejects retained direct work before incoming
activation/first mailbox reservation or deletion. Every engine state and legacy
quarantine blocks direct delivery. Outgoing-only work creates no engine/workload/PVC.

## Private delivery configuration

The optional strict `delivery` object has `nativeLeaseIssuerPublicKeyHex`,
`gatewayPeers`, `nativeAccounts`, and optional `providerBinding`:

- The native lease issuer is an explicit public key, independent of the generic
  invocation issuer. No Core database/RLS secret enters this process.
- `providerBinding`, when explicit, uses shared fields `pluginID`, `generation`,
  `descriptorSha256`, `workerID`, `workerGeneration`, `workerCredentialSha256`.
  Plugin/worker must be `mail`; both generations equal main generation; descriptor is
  SHA256 of the exact raw manifest file. The credential is the actual enrolled hash.
  For normal installer mTLS, omission derives these values from the main generation,
  raw manifest bytes and own leaf DER only when listener/callback leaf DER are equal.
  Unix/manual/split-certificate enrollment requires an explicit binding. A Unix UID
  hash is never guessed. Core and the signed native lease recheck the enrollment.
- `gatewayPeers` uses existing `CorePeer` exact leaf-pin/UID shapes, at most16,
  separately from `corePeers`; overlap is rejected. An empty list disables native
  HTTP ingress. Trusting the same CA alone does not authorize a gateway.
- `nativeAccounts` is an operator-reviewed map of at most4096 unique customers.
  Each entry has `customerID`, nullable `accountID`, `mode` (`managed` or `byok`),
  `transportRevision`, `bindingRevision`, nullable `byokRevision`, and `transport`
  with the existing private Platform shape. All marker fields, including nulls,
  must match exactly. Missing entries fail closed; native keys never borrow an
  arbitrary project's relay. The map creates no provider/account resources.

Project actions retain project relay/platform selection. The pinned private policy is
SHA256 of canonical JSON `{version:1,selection:"project",projectID,relayID,transport,
platformSha256}`. `transport` is `byok` for the immutable relay UUID, otherwise
`platform`; platformSha256 is then SHA256 of canonical effective private Platform
serialization, including changed host/port/credentials. For BYOK it is null.
Snapshot serialization uses the listed field order (typed struct), independent of JSON map features. Platform fields serialize as kind then host/port or consumerKey/consumerSecret. No credentials or private-policy hashes are logged. An old staging/queued job cannot
silently adopt a changed private policy.

Four native ingress slots retain the16MiB public request and25MiB compiled/payload
limits. Encryption/decryption has four bounded blocking slots. MIME/base64/JSON and
PostgreSQL driver copies can overlap; reserve at least2GiB runtime memory and enough
private PostgreSQL/WAL/storage capacity for retained uncertain payloads. No API limit
is relaxed to fit a smaller Pod. Production resource sizing/cutover remains an explicit
operator prerequisite.

## Native public gateway and cutover

`spitzli-mail-gateway` is packaged alongside the plugin executable. Its private JSON
has `listen` (HTTP ingress socket), `runtime` (the existing strict CoreTls shape with
the runtime's HTTPS origin/domain, separate gateway certificate/key, CA and runtime
leaf pin), and `preauthentication:{origin,secretFile,authority}` for the existing Core
private HTTPS gateway secret/CA. The gateway uses that secret only for fixed
`send.authenticate` before polling any public request body. It streams one bounded
request to the server-owned runtime endpoint over pinned mTLS, without redirects,
proxies, automatic retries, internal paths or caller-selected URLs. The runtime
rechecks both actual gateway peer and native key before consuming its body.

Generic PluginRuntime gRPC and native `/v1/emails` POST/status GET/cancel DELETE share
the existing8443mTLS listener; there is no second Pod port or new ingress wire protocol.
Original native path/query/header rejection, security headers, source-scoped IDs,
attachments/CC/BCC/scheduling and public staging409 behavior remain. Operator-reviewed
runtime endpoint/account map/gateway pin activation is required before cutover. This
change does not reconfigure a live gateway, migrate provider secrets or deploy an image.

The local Task3 process tests use an explicitly controlled Host/MailControl stand-in,
a fresh owned TLS PostgreSQL database and local catcher. They prove product/process
behavior, not actual Core source/budget authority; separate real-Core acceptance and
actual OCI build remain required before rollout.


The compatibility input exception is selected only from verified invocation claims
`pluginID=mail`, `action=mail.send`, with no job claim:128KiB action JSON retains the
original30,000 Rust-character text contract. Other action/job inputs remain32KiB and
configuration remains64KiB. Only a Mail runtime declaring mail.send decodes224KiB
Invoke frames; per-field application caps still apply. NativeControl command JSON is
bounded160KiB (15second deadline, unchanged128KiB result bound) so the original
128KiB input plus bounded16KiB token/headroom fits. This does not widen the native
public16MiB/25MiB limits or the80UTF-16 native display-name profile.
