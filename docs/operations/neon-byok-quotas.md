# BYOK Neon quota policy

Neon owns capacity and billing for connected customer accounts. Core excludes
`neon` and `neon.*` from platform plan writes and customer overrides, and omits
Neon from the platform quota editor. Feature access, connection ownership,
resource reservations, replay protection and provider limits remain enforced.

Existing stored limits need a one-time configuration update after the new Core
image is deployed. This is not a schema migration. Do not change the native
migration journal or restart customer applications for this policy.

1. Verify the deployed Core image includes the plan-write filter.
2. Use the authorized Core database maintenance connection. Inventory plans and
   customer overrides containing the exact `neon` namespace.
3. Run `tools/operations/neon-byok-plan-cleanup.sql` with `psql -X` and
   `ON_ERROR_STOP=1`. Retain its before/after JSON privately as tenant metadata.
4. Verify every remaining value equals the corresponding original value with
   only Neon keys removed, and that unchanged records retain their timestamps.
5. Verify Core readiness and a BYOK attachment through the normal service path.

The transaction locks only plan configuration tables while updating them. It
preserves other provider limits, null overrides, plan assignments and resources.
Changed records identify `system:neon-byok-policy` as the maintenance actor.

After restoring a pre-policy database backup, repeat the reviewed configuration
update before reopening plan management. A fresh installation receives no Neon
limits through the current plan-writing API.

## Local verification

The integration test creates its own disposable loopback PostgreSQL fixture:

```sh
cargo test --locked -p spitzli-plugin-host --test neon_databases -- --ignored --test-threads=1
```

It checks cleanup of both existing plan and override values, new plan writes,
retained non-Neon limits and the normal attachment/receipt/authority lifecycle.
