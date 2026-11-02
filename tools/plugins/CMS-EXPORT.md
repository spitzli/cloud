# Standalone CMS export and existing TypeScript plugins

`export_cms.py` exports the existing `managed-cms` **TypeScript protocol-v1**
plugin and its separate CMS instance into one independent source workspace. It
does not publish, change Git, copy credentials or activate a customer instance.
It does not use the Rust SDK and does not claim native-v2 compatibility.

```sh
python3 tools/plugins/export_cms.py --output /new/path/webdock-extension-managed-cms
cd /new/path/webdock-extension-managed-cms
bun install --ignore-scripts
# Commit the generated bun.lock with this independent repository.
bun install --frozen-lockfile --ignore-scripts
bun run check
bun run test
bun run build
```

Execute compilation/container builds only on verified EU infrastructure. The
exporter itself performs source preflight/copy only and requires a new output
path outside Core. It stages the result and refuses existing destinations,
selected-source symlinks/hard links, secret-shaped contents and dependencies
outside the reviewed package closure. `export-lock.json` lists exact file hashes,
withheld platform tests and the pending independent lockfile generation. It is
source provenance, not a signed or published release receipt.

The closure is `extensions/managed-cms`, `apps/cms-instance`, and these packages:

- `plugin-sdk`: existing TS-v1 contracts/server; compile its distribution first.
- `cms-ui` and `cms-bridge`: editor, scoped bridge and transfer contract.
- `payload-sso` and `oauth-session`: instance-owned OAuth/session integration.
- `instance-kit`: Payload collection access helpers.
- `i18n`: source and compiled English/German catalog dependencies.
- `database-runtime`: verified PostgreSQL connection configuration.

The three existing Drizzle/S3/cloud-storage patches and root license notices are
included. The miniworkspace retains the original layout, so the CMS Next config's
`outputFileTracingRoot`/Turbopack root and migration-builder paths resolve within
this export. No framework/config semantics were rewritten. The installed Next
`output` guide confirms that standalone tracing must include shared files and
that static assets must be copied separately; both generated Dockerfiles follow
the existing runtime layout and copy only compiled output into the runtime.
Both builder/runtime bases use the existing digest-pinned Bun 1.4.3 slim image.

The plugin and per-project CMS are **two distinct images**, built from the
export root with `extensions/managed-cms/Dockerfile` and
`apps/cms-instance/Dockerfile`. Publication requires both actual immutable
images, a real generated plugin manifest and separate registry/host admission.
Payload remains `4.0.0-canary.37`; export/typecheck is not production acceptance.
No Core Rust crates, provider connections, operator kubeconfigs, environment
files, private keys, databases, retained media, node_modules or compiled state
are copied. `packages/cms-bridge/tests/postgres.test.ts` requires the website's
migration and is withheld. The plugin image-smoke requires the Core installer
and is withheld. Database/image/backup acceptance remains an explicit disposable
fixture exercise; nothing runs against tenant data automatically.

On 2026-11-01 the real export selected 175 source files; an isolated install
resolved 370 packages and the SDK/plugin typecheck passed. No CMS Next production
build or Docker build was started. After the EU-location reminder, no further
local compilation was run; full unit/production-build/image acceptance is pending
on the owner's verified EU build host. The exporter's three Python boundary
tests pass without compiling application code.

## Existing sibling repositories — read-only diagnosis, 2026-11-01

Build/tests ran in temporary source copies with sanitized environments and the
repositories' existing installed dependencies; sibling source files were not
changed. These checks establish current local source compatibility, not a fresh
lockfile install, container acceptance or compatibility with the Rust-v2 host.
They were already completed before the explicit verified-EU-location reminder;
no further local application compilation followed it.

| Existing repository | Current version | Build / tests | Publication classification |
| --- | --- | --- | --- |
| `webdock-extension-project-notes` | `0.1.0-alpha.2` | Pass / 5 pass | Public active TS-v1 prerelease; preserve name and identity. |
| `webdock-extension-lumaserv` | `0.1.0-alpha.2` | Pass / 24 pass | Public active TS-v1 prerelease; preserve separately from Rust compute provider. |
| `webdock-extension-managed-analytics` | `0.1.0-alpha.3` | Pass / 9 pass | Public active TS-v1 prerelease. |
| `webdock-extension-managed-apps` | `0.1.0-alpha.1` | Pass / 15 pass | Public active TS-v1 prerelease. |
| `webdock-extension-managed-registry` | `0.1.0-alpha.1` | Pass / 15 pass | **Withdrawn** explicitly in README. GitHub historical assets remain; registry integrity returns 404. Never republish. |
| `webdock-extension-job-demo` | `0.1.0` | Pass / no test script | Private fixture, no GitHub releases or registry integrity; not a public product release. |

GitHub metadata confirms the first five repositories are public and job-demo is
private. The four active public releases have both `tabularium.json` and the
matching `<slug>-universal.json`. Downloads were byte-identical and matched the
registry's `manifest_raw` and reported signed SHA-256, including the immutable
GHCR image. Their registry integrity reports JWS kid `5d8025c66487bb44` and the
expected descriptor asset. This inventory did not independently establish a
trusted key or perform new pinned-key signature verification; the release gate
must verify against the owner's established pin before admitting a release.

| Slug | Exact observed manifest SHA-256 |
| --- | --- |
| `project-notes` | `8ba299bd12f41e43033b5389815f1d1f54f85541e36d77a363b36799ab0d1190` |
| `lumaserv` | `cfe86230939cca974cecbd683c85d09f353222ca561278f951d312a3d7413c32` |
| `managed-analytics` | `8b38e860297fac2e10d9f241971f14882c23e97e5f9d49490028462e24a0e8f2` |
| `managed-apps` | `03369c7843a82969162c6696e0dd6405e8b8751b6f0aff696fd558244ae94b97` |

Public prerelease assets and unit-test success do not make these protocol-v1
runtimes installable through the native Rust protocol-v2 installer. Preserve
those existing releases and repository names; a real protocol migration needs
implementation and host acceptance, rather than editing version fields.
