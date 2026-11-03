# Plugin source ownership

Canonical independent native repositories are under [Spitzli](https://github.com/spitzli):

| Directory | Repository | Contract |
| --- | --- | --- |
| archives | [cloud-plugin-archives](https://github.com/spitzli/cloud-plugin-archives) | Native extension v2 |
| echo-demo | [cloud-plugin-echo-demo](https://github.com/spitzli/cloud-plugin-echo-demo) | Native extension v2; demonstration |
| github-repository | [cloud-plugin-github-repository](https://github.com/spitzli/cloud-plugin-github-repository) | Repository provider v1 |
| hosting | [cloud-plugin-hosting](https://github.com/spitzli/cloud-plugin-hosting) | Native extension v2 |
| lumaserv-compute | [cloud-plugin-lumaserv-compute](https://github.com/spitzli/cloud-plugin-lumaserv-compute) | Native extension v2; read-only |
| mail | [cloud-plugin-mail](https://github.com/spitzli/cloud-plugin-mail) | Native extension v2; gateway included |
| mail-relay | [cloud-plugin-mail-relay](https://github.com/spitzli/cloud-plugin-mail-relay) | Mail provider v1 |
| object-storage | [cloud-plugin-object-storage](https://github.com/spitzli/cloud-plugin-object-storage) | Native extension v2 |
| neon | [cloud-plugin-neon](https://github.com/spitzli/cloud-plugin-neon) | Native extension v2; Core-owned existing-database attachments; release qualification in progress |
| postgres | [cloud-plugin-postgres](https://github.com/spitzli/cloud-plugin-postgres) | Native extension v2; legacy maintenance only |

Shared runtime/protocol crates are independently published in [cloud-plugin-sdk](https://github.com/spitzli/cloud-plugin-sdk). Consumers pin an exact Git commit. Core continues owning customer identities, connection/BYOK metadata, grants, scheduling and deployment authorization.

These directories remain compatibility/test/UI snapshots during the incremental migration. They are not the release build context for the independent repositories. Preserve Core-connected Archives acceptance tests and existing callers until replacement imports are qualified; do not delete working source merely because a repository or image exists.

Managed CMS has its own [cloud-plugin-managed-cms](https://github.com/spitzli/cloud-plugin-managed-cms) miniworkspace and separate plugin/instance images. Established `webdock-extension-*` repositories retain their protocol-v1 contracts. Withdrawn Managed Registry and private fixture repositories must not be promoted as new products.

The maintained exporters and release templates live in `tools/plugins/` and `tools/plugin-release/`. Qualification and final release evidence are recorded in [the operations report](../docs/operations/independent-plugin-releases-2026-11-01.md). Registry signatures and protocol-specific runtime binding remain required; source publication is not customer activation.

On 2026-11-02 the owner withdrew `postgres` and four legacy entries from the public catalog, preserving existing installations and data. Compatibility sources remain here for maintenance/tests. The nine retained catalog entries now have publicly verified icons, README and screenshot galleries; see [presentation acceptance](../docs/operations/2026-11-02-plugin-catalog-presentation.md). Neon is an independent stateless plugin, with Core retaining encrypted connection and attachment metadata; publishing and activation require the separate Neon qualification gates.
