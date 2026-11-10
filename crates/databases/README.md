# Native database authority

## Retired new orders

Core now defaults `WEBDOCK_DATABASE_NEW_ORDERS_ENABLED` to `false` (also when absent).
Only the exact lowercase `true` re-enables the retained shared/self-hosted PostgreSQL
new-order flow for migration or owned legacy fixtures; any other configured value
except `false` fails startup. This setting is independent of `WEBDOCK_FEATURES` and
`WEBDOCK_DATABASE_PUBLIC_ENABLED`. Keep the `databases` feature enabled to retain
viewer, credentials, bindings, exports/backups and maintenance access. The public
ordering flag alone cannot override retirement.

Core applies the setting to its database domain service and publishes the explicit
`databaseOrderingEnabled` boolean in `/console-config.json`. Console hides the new
order action and any open new-order form when false, while retaining existing
database management and upgrade controls. Its notice directs customers to create a
project in their own Neon account; it does not claim that Neon connection setup is
already available in Console. Older standalone Console fixtures without this new
runtime field keep their previous behavior.

`DatabaseService::with_new_orders_enabled(false)` rejects fresh ordinary quotes
(`databases.offers.create` without `databaseID`) and acceptance of an unaccepted
ordinary create offer with `409`, after current scope/RLS authorization. An old
unaccepted quote cannot bypass retirement. Previously persisted quote receipts can
still replay, and accepted orders replay both their original receipt and a new
idempotency key without allocating again. `databases.create` only resolves an
already accepted matching order. Existing-database upgrade offers and acceptance,
queued or running jobs, database browser access, backup/export and maintenance
remain on their existing paths. Trusted operator pool/installation repair remains
available. No storage, credentials or persisted customer records are changed by
this setting. The library constructor keeps ordering enabled for compatibility;
Core explicitly supplies its fail-closed startup setting. The export-only plugin
service has no management/order entrypoint.

Both Console and `/v1/databases` invoke the same service admission. Native MCP
currently exposes database **read** commands only; quote/accept are outside its
schema. This change does not activate MCP writes or recovery/provisioning paths.
No production deployment or configuration activation is implied by this source
change, and the Neon alternative must be integrated before the policy rollout.

`DatabaseService` owns retained registered database bindings, explicit user grants, immutable runtime ownership, and registered/managed gateway launches. SeaORM uses three separate restricted pools: `webdock_auth_runtime` for signed current-user business transactions, `webdock_auth_bootstrap` for canonical identity/session revalidation, and `webdock_worker_runtime` for the signed `database-gateway` purpose. The private allocator uses its own pool and supplies every new canonical ID.

The constructor accepts those pools, `Arc<IdentityService>`, optional `Arc<BrowserSessions>`, `Arc<IdAllocator>`, the retained RLS secret, retained application encryption secret, and gateway origin. It verifies pool roles. Public operations are:

- `execute(&Principal, &StoredAuthority, serde_json::Value) -> Result<Value>`: `list`, `get`, `create`, `grant`, `revoke`, `disable`, `open`, `openManaged`, with the retained DTO fields in `contracts.rs`.
- `exchange(code) -> Result<Value>`: consumes a hashed 60-second launch once, atomically inserts a hashed opaque 15-minute gateway session, and returns `{token,scopeID,expiresAt}`.
- `inspect(token) -> Result<Value>`: confidential retained registered or managed gateway DTO. Credentials must never be returned to Console.
- `logout(token) -> Result<Value>`: idempotently removes matching gateway sessions, including when the module is disabled.

`StoredAuthority` is shared from `spitzli-browser-session`. It is a server-owned reference, not an HTTP authentication DTO. First-party browser records preserve exact confidential client/origin/session references, role and scope ceilings, bounded expiry and read-only state. Every inspection revalidates those against current storage as well as the canonical user/session. No opaque OAuth token becomes a central session. Current bans, setup state, externally mapped identity/organization activation, membership, preview and resource permissions are checked again. Operators require an explicit personal grant to open registered databases. Managed database viewers require current database-management permission and confirmed backend/credential revisions.

The additive migration `migrations/0001_native_authority.sql` is frozen as native journal version 0014. It adds independent native proof columns and exclusive-source shape constraints to the four retained launch/session tables. External-provider rows are never interpreted as local proof. Existing native rows with a canonical session and no external proof remain bounded by their original expiry. New rows always record native authority.

Grant secrets and confirmed managed credentials use `spitzli-secrets`' exact retained `v1` AES-GCM envelopes, with purposes `database:<binding>:<subject>` and `database:<database>:<confirmed_revision>`. Changing privileges requires a different isolated runtime origin; a retired origin cannot be reassigned to a different binding/user/profile. PostgreSQL roles and the isolated runtime enforce SQL privileges; this service does not classify SQL text.

Core mounts browser commands at `/api/console/databases` (Console/Studio) and `/v1/databases` (API), with the canonical Auth `/api/databases/gateway` machine endpoint. Gateway bearer-secret comparisons are constant-time. Origin/host checks, bounded strict JSON, module admission and server-side actors apply before execution. Configure `WEBDOCK_DATABASE_GATEWAY_ORIGIN`, `WEBDOCK_DATABASE_GATEWAY_SECRET`, and the separate restricted session/worker URLs. `WEBDOCK_HOSTING_ENCRYPTION_KEY` retains the existing application cipher secret, falling back to the retained authentication secret when no separate key was configured.

Console reuses the retained `@tabularis/web-ui` workbench through its explicit launch-only React boundary. The geometry dependency loads the standard `buffer` browser shim only with the workbench. Locale preferences use the existing i18n catalog and a host-only preference cookie. Resource IDs in query strings remain strings, including signed 63-bit values. The UI CSP permits bundled data fonts and same-origin/blob editor workers; gateway HTTP origins and their exact WS/WSS equivalents must be explicitly listed in `WEBDOCK_UI_CONNECT_ORIGINS`.

Verification: contract tests and the ignored `persistence` test against the owned literal-loopback `webdock_rls_test` fixture cover real MFA operators, explicit grants, isolation changes, retired-origin ownership, preview denial, code replay, malformed persisted proof rejection, grant revisions, membership loss, logout, canonical session loss, managed confirmation/revision and endpoint readiness. `apps/console/tests/database-browser.mjs` exercises the Rust-served Console, native first-party SSO, real gateway, TLS-protected isolated SQLite runtime and retained editor. The fixture TLS proxy is test infrastructure only.

Native runtime execution and full migration/reconciliation end-to-end proofs remain coordinated with the agent service. Backups/recovery remain unconfigured at the retained source floor. Optional global keybinding/plugin discovery requests in the retained embedded workspace remain denied; they do not grant host-file/plugin privileges. Full account/Console translation coverage and physical passkey verification remain platform parity work.

## Managed control and execution

`with_management(ManagementConfig)` enables the retained catalog/order policies. Core reads `WEBDOCK_DATABASE_CLUSTER_ID`, `WEBDOCK_DATABASE_DOMAIN`, `WEBDOCK_DATABASE_PUBLIC_ENABLED`, `WEBDOCK_CONTROL_DATABASE_ID`, and the retained lease signing secret. Public ordering remains disabled unless explicitly enabled; pilot plans require an operator and sponsorship is bound to its customer.

`execute_managed(&Principal, &StoredAuthority, Value)` validates the retained managed command schema and handles project permissions/listing, immutable plan versions, availability, offers/acceptance, shared installations, pool registration, database views, upgrades and maintenance. Accepted orders use a separately signed, customer-bound `database-order` transaction after current browser/session authorization. Capacity admission includes observed unknown allocations, held physical reservations, application/native demand, node headroom, and current qualified pool observations. Same-filesystem CPU/memory increases queue a resize. Changing volume size or moving shared to dedicated allocates a new backend and a credential-bound migration, retaining the source. Fixed ext4 volume growth is never represented as an in-place quota change. Billing starts only after a confirmed ready completion.

Pause/resume/restart/rotation/deletion/purge and reconciliation retain revision/idempotency fences. Deletion previews are short-lived and bound to the session, exact name and revision; purge requires expired retention. Previously claimed uncertain work is only reconciled against an existing observed cluster identity; unclaimed provisioning can be retried under the narrow retained conditions. Backup, restore and clone deliberately preserve the existing `409` unavailable floor.

`display_credentials(principal, authority, database_id)` is the separate audited, write-authorized credential display operation. It requires confirmed revisions and is not a gateway token response. `binding_material_in(tx, &DatabaseBindingRequest)` supplies only the application role to an already authenticated and authorized Hosting operation, with the exact CA mount and `sslmode=verify-full`; callers must validate the persisted app/binding revision in the same transaction. No material belongs in durable dispatch or logs.

The agent APIs consume `DatabaseTransaction` from `HostingService::begin_authenticated_agent` and the opaque enrolled `AgentIdentity`: `claim_database_operation`, `guard_database_operation`, `checkpoint_database_operation`, `complete_database_operation`, `database_observation_config`, and `report_database_observations`. The shared pure DTOs and SHA-256 specification fingerprint live in `spitzli-agent-protocol::database`. `validate_database_callback_in` binds callbacks to issued safe specs; checkpoints use `validate_database_checkpoint_in` so a committed exact ACK can clear an expired outbox without renewing its lease. Durable issued specs and receipt hashes permit lost-Poll/exact-ACK redelivery; secret material remains ephemeral. Agent enrollment generation and operation generation are distinct. Expired guards never authorize execution, and a crash during credential configuration remains uncertain.

Restricted persistence tests additionally cover new orders, same-volume resize versus migration, required interruption consent, unchanged source filesystem, retained credentials, maintenance previews/retention, conflicting work, lost Poll delivery, immutable safe specs, checkpoint order, expired exact ACKs without renewal, terminal exact replay, signed observation UID checks, and fixed-CA service material. Core accepts these commands at the existing browser/API database endpoints. Console uses the retained schema, explicit order review and credential panel, with server-owned first-party authority throughout. `database-management-browser.mjs` separately verifies browser order admission against synthetic local capacity; it does not claim provider provisioning.

Pool acquisition is ordered `worker → authentication → business` for both accepted orders and agent callbacks. An order reserves its worker transaction before holding identity/business connections, but installs no signed worker scope until authorization determines the exact customer. Other browser commands need only authentication/business. The real regression saturates each role pool at one connection, starts an order behind an already-held agent worker transaction, and proves both requests complete without larger pools or longer timeouts. Callback validation also acquires the cluster advisory lock before operation/job row locks; the real PostgreSQL test proves a blocked callback does not hold those rows.

`capacity_options(principal, authority)` returns only operator-authorized platform cluster and qualified pool metadata. Core exposes it as the strict facade action `databases.capacity`; the retained managed command schema stays unchanged. Console uses it to register observed storage pools and create shared installations with the same revision/idempotency fences. No global plugin/agent permissions were widened. Migration tests now complete preparation, copy-checksum-bound cutover, exact old preparation ACK replay, retention-expiry admission, and retained-source purge with credential erasure. These are real restricted persistence tests with synthetic execution proofs; the native agent's actual provider execution remains a separate verification gate.
