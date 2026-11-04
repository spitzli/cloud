# Spitzli Cloud package and deployment names

## Source license

The current change licenses first-party Spitzli Cloud source under Apache-2.0.
Root LICENSE, NOTICE, contribution guidance, Rust workspace metadata, JavaScript
metadata and the CLI's first-party SDK scaffold agree. Public SDK/CLI package
archives include LICENSE and NOTICE. Separate component licenses, including the
Mintlify documentation template, Maylily and independently licensed plugins,
remain in place. Earlier published artifacts retain their original license files.

## Active Vercel project names

The existing projects were renamed by immutable project ID on 2026-11-04:

| Previous name | Current name |
| --- | --- |
| webdock | spitzli-cloud |
| webdock-auth | spitzli-cloud-auth |
| webdock-admin | spitzli-cloud-console |
| webdock-cms | spitzli-cloud-cms |

The rename did not replace production deployments or change function regions.
Canonical domains remain spitzli.cloud, auth.spitzli.cloud and
console.spitzli.cloud. New deployment integrations can use the new names;
historical GitHub deployment records still describe the name used at the time.
No environment secrets, protection rules or deployment history were deleted.

## Verified current publication

PR #69 is merged on `main`; GitHub recognises Apache-2.0 for the Core repository.
The public npm packages `@spitzli/plugin-sdk@0.1.0` and `@spitzli/cli@0.1.0`
are published with verified archive integrity, Apache licensing and their notices.
A fresh anonymous registry install of the CLI resolved its SDK dependency without
local overrides; both executable aliases and both template install/build/validate
flows passed.

The Core image is public at
`ghcr.io/spitzli/cloud@sha256:a7ef2578f933ebddd5d02b576c157b69cc07372e6d0ef9a8ff8a5f75a95f777f`.
It was built from licensing commit `bccb838f`; the later npm namespace commit
changes no native Rust, Cargo, Docker or vendor inputs. Actual OCI executables
passed initial/repeated migrations and readiness against an owned disposable
PostgreSQL 18 fixture. The pushed digest matches the qualified artifact and its
manifest/configuration are readable anonymously. Production was not rolled out.

See [the publication receipt](../verification/2026-11-04-apache-package-publication.json).
This is a later phase than the source qualification record that correctly marked
publication as pending at that time.

## Container publication

The current backend bootstrap configuration targets `ghcr.io/spitzli/cloud`.
Its public artifact name is `spitzli-cloud-backend-bootstrap-<source SHA>`.
The internal bootstrap archive filename remains stable for existing consumers.
Future backend OCI metadata identifies Spitzli Cloud, Apache-2.0 and
`https://github.com/spitzli/cloud`.

The old public `ghcr.io/spitzli/webdock/...` images and charts still exist.
A source metadata update does not rename registry objects, republish old bytes
under a new license or prove that a new image has been built. The current qualified Core image is published above. Inventory retained pull/rollback
references and migrate consumers before retiring old addresses. Keep existing
signed descriptors, immutable digests and rollback material intact.

Internal Rust crate names, environment variables, Kubernetes identities and
protocol names retain compatibility. The public TypeScript authoring packages are now named `@spitzli/plugin-sdk` and
`@spitzli/cli`. The primary CLI executable is `spitzli-cloud`; `webdock` remains a
compatibility alias. Consumers, lockfiles, scaffolds and export tooling use the new
scope. Source migration and tested archives do not establish npm publication: verify
the exact published version and integrity through the registry before reporting it
available. The native Rust SDK and its protocol identities remain unchanged.
