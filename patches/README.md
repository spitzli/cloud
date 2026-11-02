# Drizzle 0.45.2 peer identity

The pinned Payload 4 canary uses Drizzle 0.45.2 from both `@payloadcms/db-postgres`
and `@payloadcms/drizzle`. Bun resolves their optional `pg`/`@types/pg` peers
in different contexts. TypeScript consequently treats otherwise identical `SQL`
classes as different private identities, breaking the existing migration contract.

The patch removes only those two optional peer declarations and their optional
metadata. It changes no JavaScript or SQL. Payload's PostgreSQL adapter still
owns its explicit PostgreSQL dependencies. Keep the current Payload, Drizzle and
PostgreSQL versions; remove this patch when an upstream release resolves the
identity conflict. Verify with a fresh non-incremental CMS typecheck, existing
migration acceptance and the Next production build. Root-context Bun Docker
builds must include this directory before installation.

# Payload S3 4.0.0-canary.37 upload result

The S3 adapter returns the complete input document from `handleUpload`, despite
adding no provider metadata. Cloud Storage treats that return as new metadata and
performs a second database update. The update changes `updatedAt` while the create
response retains its original value, so the next legitimate CMS compare-and-swap
edit fails with 409. It also feeds unchanged lifecycle fields back through access
checks. The one-line runtime patch returns no metadata after the successful upload;
all provider calls, errors and the normal document transaction remain intact.
The real S3 editor acceptance is the regression test. Remove this patch when the
matching upstream adapter returns only actual provider metadata (or none).

# Payload S3 4.0.0-canary.37 read cancellation

`getFile` passed no AbortSignal to HeadObject and ignored an already-aborted
caller. The runtime patch applies cancellation to HEAD and GET, destroys a
returned body on cancellation, and sets a 30-second deadline covering HEAD,
retries, GET and body consumption. It removes the caller listener and timer on
completion, cancellation and error. The stream error listener is installed before
handling a cancellation racing with body arrival. CMS also enables Smithy's
`throwOnRequestTimeout` for its 15-second request timeout; otherwise that setting
only emits a warning. Caller cancellation or a shorter deadline wins.

`storage-dependencies.test.ts` exercises the installed dependency with stalled
real HTTP HEAD/GET/body responses, an already-aborted request, the GET-body race,
and the configured adapter's full deadline without a caller signal. Socket closure
is asserted. Remove the patch when upstream covers these cases.

# Payload Cloud Storage 4.0.0-canary.37 purge errors

The official afterDelete hook logs provider errors and resolves successfully.
Payload consequently commits deletion of the document and its versions even when
DeleteObject failed. The one-line patch rethrows the original error so the active
PostgreSQL transaction rolls back, retaining the exact retryable object reference
and version history. It adds no compensating byte deletion. The native editor
continues to trash through a metadata update; permanent purge is separate.

The dependency-hook regression checks rejection and the exact object key. Real
TLS PostgreSQL/MinIO acceptance attaches a fresh delete-only Deny policy to its
owned fixture user, checks that purge rejects while metadata, versions and bytes
remain, removes that policy and retries the same purge successfully. This does
not make S3 and PostgreSQL atomic after a successful provider deletion. Remove
the patch when upstream propagates provider purge failures.
