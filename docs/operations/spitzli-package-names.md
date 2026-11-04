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

## Container publication

The current backend bootstrap configuration targets `ghcr.io/spitzli/cloud`.
Its public artifact name is `spitzli-cloud-backend-bootstrap-<source SHA>`.
The internal bootstrap archive filename remains stable for existing consumers.
Future backend OCI metadata identifies Spitzli Cloud, Apache-2.0 and
`https://github.com/spitzli/cloud`.

The old public `ghcr.io/spitzli/webdock/...` images and charts still exist.
A source metadata update does not rename registry objects, republish old bytes
under a new license or prove that a new image has been built. Qualify and publish
the new source image to the current namespace, inventory retained pull/rollback
references, then migrate consumers before retiring old addresses. Keep existing
signed descriptors, immutable digests and rollback material intact.

Internal Rust crate names, environment variables, Kubernetes identities and
protocol names retain compatibility. The public npm scope migration is a separate
package-consumer change; it must include imports, workspace dependencies, lockfiles,
exports, scaffolds and publication verification rather than just changing a label.
