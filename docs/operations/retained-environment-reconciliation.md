# Retained application environment reconciliation

The native agent normally owns immutable environment Secrets named
`app-<id>-env-<revision>`. Cleanup must never infer ownership from a name alone:
project, application and cluster labels and a positive nonfuture metadata revision
are verified first. Canonical unused revision Secrets retain the existing live
lease check and UID/resourceVersion deletion preconditions.

An operator-maintained environment can already be referenced by a retained
workload under `app-<id>-env-maint-<id>`. Previously cleanup parsed that suffix as
a revision before checking its references, so a correctly owned running application
could fail an update or stop with `ownership_conflict`.

The narrow retained-material exception accepts only lowercase hexadecimal
maintenance IDs of 6–64 characters, with full current owner/revision checks, Opaque
type, immutable=true and nonempty UID/resourceVersion. Such Secrets are retained,
even when no longer referenced; this does not adopt their contents, authorize a
new reference or make them garbage-collection candidates. Their eventual deletion
requires explicit operator retirement. Unknown names, wrong owners, mutable
maintenance material and malformed/future revisions remain rejected. Service
credential handling is unchanged.

Before retrying a failed desired operation, compare the current live image,
environment, storage and resources with the saved desired state. A healthy Pod
can contain a newer out-of-band release while Core still records an older failed
request. Do not replay that old request as a cosmetic status repair, change owner
labels to make it pass, or directly rewrite its success status.

Prepare an explicit revision-bound native update only after the latest intended
configuration is established. Pin the current workload UID and configuration,
keep a protected current-runtime rollback snapshot and previous managed history,
and use the normal operator API with a stable idempotency key. Reject concurrent
or unexplained differences. Preserve existing Pod/PV/PVC resources and credentials;
validate the agent artifact before rollout and independently check readiness after
any accepted application operation.

Local qualification:

```sh
cargo test --locked -p spitzli-agent --lib --all-features -j2
cargo clippy --locked -p spitzli-agent --lib --all-features -j2 -- -D warnings
```

Three new selection regressions cover referenced and retired owned maintenance
material, canonical positive cleanup, foreign ownership, mutable material,
missing UID/resourceVersion and malformed/future revisions. The exact old parsing
failure was reproduced before implementation. These tests do not replace signed
source, Blacksmith CI, the EU binary/ABI/configuration qualification or production
runtime checks. No ownership, storage, privilege or public-build gate is relaxed.

## Lunares control authority and qualified handover

On 2026-11-05 the owner confirmed Spitzli Cloud is the deployment authority.
External Lunares rollouts are a temporary workaround, not a second permanent
controller. Pause that writer before the one-time handover.

The normal operator API superseded failed revision37 with revision38 containing
the reviewed running immutable image and all32 existing environment values.
Credential values stayed in memory until the normal Core encryption path; no
plaintext GitHub token was written to the local request file. Execution was
rejected with `capability_unavailable`, and the protected runtime snapshot
remained unchanged and healthy. A subsequent independent external rollout changed
the image again, making revision38 outdated as well. Do not retry either revision.

The retained Pod template contains the one-off `lunares-data-migration` init
container. The native retained-Pod profile deliberately rejects init containers.
Its exact reviewed source, successful exit, matching completion marker, retained
backup and read-only SQLite `quick_check` were verified. Do not relax that general
guard, execute the migration again, or delete business data. Retire only that
completed initializer using exact workload UID/resourceVersion/spec tests, with
the root-only rollback snapshot retained. This template change can restart the
Pod. Recheck the latest image, environment, storage, live source and controller
revision before a new ordinary operator update; record independent readiness.

See [the release evidence](../verification/2026-11-05-schattenclan-release.json).
A prepared local retirement command is not evidence that retirement or handover
has happened. Public Git/build admission remains closed.

### Completed handover

After the owner confirmed manual rollouts were paused, the latest running source
`91360731f24967bf9690c6924a47db65342288aa` and immutable image
`webdock.local/lunares@sha256:ac79ca7a78de9164c02131ae676d5b21b3093d20de68d9c8ee7bb1bca13aa580`
were pinned. All32 live environment values matched the encrypted Core state.
The completed initializer was removed with UID/resourceVersion/full-spec JSON
Patch tests after repeating its completion-marker, retained-backup and read-only
SQLite integrity checks. Only the initializer was removed from the Pod template;
no business-data migration was executed. The template change rolled the Pod.

A new normal operator `apps.update` admitted revision39, preserving the existing
encrypted environment. Operation `111938270526447616` succeeded: desired39 and
observed39 agree, status is `ready`, and the previous error is cleared. An
independent runtime read verified the image, all32 environment values in immutable
`app-102047516467400704-env-39`, and unchanged Deployment/PV/PVC/Service UIDs.
Old environment sources and protected rollback snapshots remain retained.
Revisions37/38 are failed historical operations and must not be replayed.

The genuine Schattenclan customer role read the ready39 app and executed the
limited database-viewer metadata query successfully. Own customer mode and both
verification sessions were closed afterwards. Native init-container, ownership,
storage and admission checks remain unchanged. This verifies controller-managed
runtime deployment; it does not complete separate Git-push onboarding or the
Admin-Panel's Discord-user acceptance.
