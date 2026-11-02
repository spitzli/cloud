# Mail metadata backfill and Relaykit import

Both tools are offline Bun programs. Neither creates a provider account/key,
changes DNS, starts an executor or imports identity/membership records. Run with an
explicit target `host:port/database`; keep environment files and source data outside
the repository. Reports contain counts, IDs and checksums only.

## Existing Webdock data

From `apps/auth`, with gateway admission off:

```sh
bun --env-file=/private/migration.env scripts/mail-backfill.ts --dry-run HOST:PORT/DATABASE /private/byok-mapping.json
bun --env-file=/private/migration.env scripts/mail-backfill.ts --apply HOST:PORT/DATABASE /private/byok-mapping.json
```

The optional mapping file is an array of
`{ "customerID": "…", "providerAccountID": "…", "sourceRevision": 1 }`.
Managed accounts use their existing provider IDs. A BYOK source without an existing
verified binding requires an explicit original account ID and current source
revision; unresolved sources are reported, never guessed from a Consumer Key hash.
Source rows are locked while copying metadata. Managed sender/tracking bindings
are preserved; BYOK provider snapshots alone do not establish domain ownership.
New bindings remain pending without private sending credentials. Repeat execution
reuses existing bindings. The legacy tables and direct SMTP credentials remain.

## Optional Relaykit source

`RELAYKIT_READONLY_DATABASE_URL` must reference the frozen original source. Use a
read-only database role; the program additionally enforces a read-only,
repeatable-read transaction and statement timeouts. No CSV/body exports are written.

```json
{
  "version": 1,
  "sourceID": "relaykit-original-instance",
  "providerAccountID": "confirmed-original-provider-account",
  "executorStopped": true,
  "accounts": [
    {
      "legacyAccountID": "original-resource-account",
      "customerID": "confirmed-webdock-customer",
      "historicalConnectionID": "confirmed-original-provider-connection"
    }
  ]
}
```

The ID examples above must be replaced with actual numeric Webdock/provider IDs.
`legacyAccountID` denotes Relaykit's resource account, including an account attached
to an organization; it is not automatically a current user. Every source resource
account must be mapped. Relaykit's provider credential is instance-wide, so the
standard importer refuses to split it across Webdock customers or historical
connections (`shared_provider_split_required`). Do not weaken provider-account
uniqueness to force such an import.

Stop old admission, scheduled sends and webhook executors before asserting
`executorStopped`. Keep both Webdock gateway and webhook delivery off, and the
original destination connection paused. Run:

```sh
bun --env-file=/private/migration.env scripts/mail-import-relaykit.ts --dry-run HOST:PORT/DATABASE /private/relaykit-mapping.json
bun --env-file=/private/migration.env scripts/mail-import-relaykit.ts --apply HOST:PORT/DATABASE /private/relaykit-mapping.json
```

The import validates all account mappings before the first batch, checks payload
bounds, and commits each batch together with `mail_import_map`. Stable row checksums
reject changed source data or changed target mappings. A durable cutoff keeps
retention decisions stable during resume. Re-running a partially completed import
reuses mappings rather than duplicating messages or deliveries.

Existing verified Webdock domain claims may be reused within the same customer.
Imported live claims otherwise need a fresh Webdock TXT ownership proof. Deleted
source claims receive historical, non-authoritative IDs/tombstones; they do not
restore sending authority. Retention uses original sent/terminal events, not later
opens/clicks. Minimal metadata survives when a retained pending delivery needs it.
Suppression events reconstruct excluded recipients before imported quota totals.

Queued/uncertain messages stay `outcome_unknown`, with a dedupe tombstone and
reserved allowance; they are never submitted automatically. Schedules remain
attached to the paused original account. Historical message/event IDs are remapped,
large provider MIDs remain strings, and logs are redacted. Expired content is not
renewed. Webhook secrets are purpose-bound and encrypted in the destination;
imported endpoints start disabled. Previously attempted pending deliveries become
failed for explicit review/replay because an old response might have been lost.
API keys are counted but not copied: issue new `wdm_` keys and update clients
explicitly. No old API URL is silently redirected.

Before resuming, verify counts/checksums, ownership proofs, unknown cases, original
account identity and provider callback routing. Obtain a source-consumer inventory
and a reversible callback cutover; shared callbacks need durable fan-out. Start one
executor only. Keep the source snapshot read-only for reconciliation. A schema or
source-contract mismatch is an error requiring investigation, not a reason to
assign rows to a default tenant or enable a different provider account.
