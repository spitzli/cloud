# Managed CMS

An optional Webdock plugin with a native content editor and independent CMS instances. Its control runtime is separate from each project's content database and media volume. The retired shared `apps/cms` is not used.

The plugin uses generic approved application provisioning and scoped HTTP services. Hosting chooses the reviewed image, database plan and resource limits; plugin input cannot choose commands, provider credentials or infrastructure.

## Build and test

From the Webdock repository root, with Bun:

```bash
bun install --frozen-lockfile
bun run --filter @webdock/plugin-sdk build
bun test ./extensions/managed-cms/tests
bun extensions/managed-cms/build.ts
docker build -f extensions/managed-cms/Dockerfile -t managed-cms-plugin .
docker build -f apps/cms-instance/Dockerfile -t managed-cms-instance .
```

The optional signed release build generates `managed-cms.plugin.json` from the exact tested registry root digest (`oci.registryDigest`) and includes it in the signed checksums. For a separate reviewed image, generate its manifest using its actual immutable registry root digest:

```bash
bun extensions/managed-cms/manifest.ts \
  ghcr.io/spitzli/webdock/managed-cms@sha256:ACTUAL_DIGEST \
  managed-cms.plugin.json
```

The runtime listens privately with TLS on port 3443, and serves its sandboxed UI on port 3444. It uses the installer's standard `WEBDOCK_PLUGIN_PUBLIC_KEY` PEM, runtime token, generation, host URL and TLS-file contract. Its exact Console CSP comes from operator-configured `runtime.consoleOrigin`, emitted as `WEBDOCK_PLUGIN_CONSOLE_ORIGIN`; the installer must be upgraded and configured before UI installation. The installed pod runs as UID/GID 1000, overriding the image's default 65532. No CMS credentials are embedded in the UI.

## Installation prerequisites

Use a Full installation with Hosting and Databases, migration 0048 or later, the updated managed agent, an approved CMS image/database plan, cert-manager HTTPS publication for each exact instance hostname and sufficient project capacity. Core-only installations return `capability_unavailable` for provisioning.

Before serving a new CMS image, run its bounded migration job, including `20261018_201453_managed_media_trash`. This additive migration retains existing tables and files.

Configure `WEBDOCK_APPROVED_CMS_TEMPLATE` identically on API and worker, and the matching root-owned agent approval. See [operator configuration](../../agents/kubernetes/APP_TEMPLATES.md) for its exact shape, certificate renewal, migration recovery and matched backups. Install and grant the plugin using Webdock's normal reviewed plugin lifecycle. Each instance owns its cert-manager Certificate, Ingress and TLS Secret; certificate private keys are not copied from a wildcard Secret. Wildcard DNS still routes its hostname to the approved ingress. Installation alone does not create a customer instance; **Create CMS** does.

## Content and access

- English/German pages, structured text/image sections, SEO, media and site settings.
- Readers view content; editors create, edit and publish; administrators delete and restore versions.
- Current website grants are checked on each bridge call. Local user restrictions further limit central roles.
- Documents use optimistic concurrency. An uncertain mutation is surfaced for reconciliation and is never retried automatically.
- Raster media is limited to 10 MiB and 40 megapixels. SVG, HTML and animated images are rejected.
- Existing media files are immutable; upload and select a new item to replace one. Metadata remains editable.
- Managed media removal uses native trash (`removeToTrash`): administrators hide the item from normal lists and fresh public requests while retaining its bytes. Trashed files count toward storage and backups until explicit operator purge; removal does not reclaim quota.
- Disabling the plugin revokes editing and new provisioning while published content continues to serve. Explicit app suspension stops serving and retains data. Deletion/purge is separate.

Existing website integrations keep their schema and use a PostgreSQL transfer journal across serverless requests. Run the website's `cms:migrate` before enabling the bridge and give the runtime DML access to its transfer tables. The single-instance filesystem journal is reserved for independent managed instances.

## Verification boundary

Payload is pinned to `4.0.0-canary.37`. This is a canary dependency, not a stable Payload release. Production readiness requires the complete fresh-instance, role, publication, restart and matched backup/restore acceptance, not just compilation or unit tests. Deployment evidence belongs in the release record. Local image checks do not establish cluster provisioning, certificate issuance or a successful matched backup restore.

Licensed under AGPL-3.0; separate commercial licensing is described in the repository's `COMMERCIAL-LICENSING.md`.
