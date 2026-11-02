# Native operator allowances

Core supports finite installation-operator supplements for an actual active Registry
customer. This configuration does not purchase a plan, change a paid base or payment
status, activate a customer service, contact a provider, or remove resources.

Use the existing offline operator bootstrap, replace its initial password, enroll and
confirm TOTP, and complete the native Console PKCE sign-in. Create the customer through
`POST /api/console/registry/customers`; its returned ID is the allowance target. A native
account cookie alone is insufficient: these routes require the current Console browser
session, current operator role and Console operator mapping, without a customer preview.

`GET /api/console/access/customers/{customerID}/allowances` returns the current revision
(`0` when no subscription exists), selected supplements, selected subscription-effective
values, customer-effective values after any customer Hosting ceiling, and the operator's
latest allowance receipt. Project/provider ceilings and service readiness are separate.

With the native Console cookie and exact Console `Origin`, send JSON to
`POST /api/console/access/customers/{customerID}/allowances`:

```json
{
  "expectedRevision": 0,
  "idempotencyKey": "operator-allowance-1",
  "hosting": {
    "apps": 2,
    "cpuMillicores": 1000,
    "memoryBytes": 1073741824,
    "ephemeralBytes": 2147483648,
    "volumeBytes": 4294967296,
    "replicasPerApp": 2,
    "concurrentDeployments": 1
  },
  "mailMessages": 10
}
```

All eight supplements are required nonnegative JSON integers up to 9007199254740991.
Null, unlimited values, unknown fields, roles, arbitrary base JSON and payment/plan
flags are rejected. Keys contain 1–128 ASCII letters, digits, `.`, `_`, `:` or `-`.
`expectedRevision` is a nonnegative integer below 2147483647. The response includes the
new revision, selected effective values and an immutable operation receipt. A stale
revision or a key reused for a different body/customer returns 409. After losing a
response, replay the identical command/key: current authority is checked again before
the original result is returned, even if a later operation advanced the revision.

The operation replaces only the selected `extras.hosting` dimensions and
`extras.mailMessages`. Other extras, including unsupported legacy Hosting keys, and
all existing base, plan, name, description and paid projection data are preserved.
Readback projects the selected quantities rather than returning arbitrary legacy JSON;
legacy fields may still require separate remediation by their owning product.
A new subscription has neutral metadata and an empty zero base. A missing subscription
with an unreleased projected paid contract is rejected. The existing paid-base trigger
continues to apply. An unlimited base remains unlimited after finite supplements;
`subscriptionEffective` represents that with null. Finite supplements are not a cap.

Lowering an allowance changes future admission only. Existing usage, active resources
and uncertain reservations are retained. Readiness, customer/project ceilings and
provider or sender authorization still apply independently.

## Optional first-install Mail availability

The private offline operator input accepts optional `initialMailEnabled: true` or
`false`. Omission preserves the original serialized reviewed specification/checksum
and the existing platform-default behavior. Null and non-boolean values are invalid.
An explicit choice is recorded in the reviewed setup metadata/checksum. It is applied
only during fresh offline bootstrap; a conflicting existing setting rejects bootstrap.
An exact or changed retry on an installed system is rejected and cannot change settings.

This is installation availability only. It provisions no Stalwart instance, mailbox,
forwarding, sender verification, transport, tenant allowance, role or purchased plan.
Incoming Mail remains explicitly enabled per customer. Existing installations need
their separately supported configuration procedure; this input is not an update API.

## Authority and persistence

Allowance reads, writes and receipt replay serialize with preview entry and exit for
the same native session. After acquiring that business-transaction fence, the Core
adapter revalidates real first-party browser proof inside an authentication transaction
retained through the Access domain commit. No authentication row locks are retained
while waiting for the session fence. Access checks the mapped
operator `FOR SHARE`, active native Registry customer, customer billing permission,
customer/plan locks, subscription revision and current authority before commit/replay.
Confirmed disabled canonical clients return 401. An ambiguous `InvalidClient` with an
active or missing canonical client remains 503, including a wrong protected configured
client secret; persistence/configuration failures are not presented as authorization
denials. The adapter reads only the canonical disabled flag and reuses the existing
browser verifier.

Compiled builtin activation uses the same session fence before retaining browser
authority and taking its installation lock. Existing installations and tenant grants
remain unchanged by that setup path.

Hosting admissions take customer then `plan:{customer}` locks. Mail budget admission
takes a shared customer lock before its budget/subscription locks, so allowance changes
wait for in-flight admissions. No usage or reservation table is changed by this command.

Migration 0055 adds `webdock_auth.allowance_operation` with forced RLS, native auth
INSERT/SELECT only, current operator/subject/customer billing predicates, and a trigger
rejecting UPDATE/DELETE even for its owner. Subscription history, receipt and the existing
`allowances-configured` Access audit event commit atomically. There is no worker grant.
Migration 0055 is [frozen](../verification/2026-10-26-native-operator-allowances-55-freeze.json);
never rewrite it. Migration 0054 remains a separate draft recovery prerequisite.
That recovery implementation and its acceptance are not approved by this foundation.
