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
cargo test --locked -p webdock-agent --lib --all-features -j2
cargo clippy --locked -p webdock-agent --lib --all-features -j2 -- -D warnings
```

Three new selection regressions cover referenced and retired owned maintenance
material, canonical positive cleanup, foreign ownership, mutable material,
missing UID/resourceVersion and malformed/future revisions. The exact old parsing
failure was reproduced before implementation. These tests do not replace signed
source, Blacksmith CI, the EU binary/ABI/configuration qualification or production
runtime checks. No ownership, storage, privilege or public-build gate is relaxed.
