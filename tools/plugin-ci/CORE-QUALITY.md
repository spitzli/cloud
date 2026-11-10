# Manual EU frozen-source Core readiness checks

This is the explicitly authorized manual trusted-platform qualification path on
existing Contabo EU2 BUILD (`167.86.75.144`, physical DMI
`e6d9884c-e6bd-4699-8a0c-99f2358ae8a1`). It does not start a GitHub workflow or
qualify Blacksmith. No customer/import source, production credentials, Runtime
SSH, Kubernetes API, database, deployment or Git index operation is involved.
Do not start until the parent provides the frozen archive SHA and source ref.

`core_quality.py prepare` checks a flat frozen source archive, maximum256MiB,
with exact SHA. All safe source roots are retained: Core Cargo workspace and
standalone plugins, tools/quality and other helpers, Console/frontend Bun
workspace closure, locks, patches, migrations, deployment render fixtures and
legal files. The compute process fixture `tests/fixtures/servers-panel.json`
is required. Private/runtime directories, dotenv configuration, credential
files, Git/build state, private keys, databases, unsafe links/paths and duplicate
selected entries are rejected. Internal Git source links are materialized only
against safe members within the archive. No existing context is overwritten.

Two historical files are required compile-time fixtures for existing database
tests: `docs/verification/2026-10-25-native-postgresql-backup-transitions.json`
and `docs/superpowers/plans/2026-10-26-postgresql-restore-0056.draft.sql`. Include
only these reviewed files in the private source input; this does not admit the
internal documentation tree for publication. Their exact bytes remain historical
test inputs, and the draft SQL is not a new production migration. Preparation
rejects an input that omits either fixture.

Upload unit fixtures use private directories on the existing `/dev/shm` tmpfs
for real `O_TMPFILE` operations. Overlay-backed `/tmp` can reject that operation
with `EOPNOTSUPP`. The production gateway still requires its own private,
kernel-size-limited tmpfs mount; the ordinary-disk rejection test stays enabled.

Preparation and synthetic harness tests may run on the operator workstation.
Rust/Bun application compilation runs only on qualified EU BUILD:

```sh
python3 tools/plugin-ci/core_quality.py prepare \
  --archive /private/parent-frozen-source.tar \
  --source-sha256 EXACT_PARENT_SHA256 \
  --review-ref EXACT_PARENT_SOURCE_REF \
  --output /private/new-core-quality-context
```

Transfer the compressed context to a new UID1001-owned private directory on
BUILD, using only the existing authorized local Ed25519 server SSH channel.
Recheck physical identity and qualified rootless BuildKit before executing.
Invoke the copied helper as `spitzli-plugin-ci` with its qualified PATH/HOME:

```sh
python3 CONTEXT/core_quality.py build \
  --context CONTEXT --source-sha256 EXACT_PARENT_SHA256 \
  --mode focused --output NEW_PRIVATE_FOCUSED_OUTPUT
# Only the parent-approved final source proceeds to the complete gate.
python3 CONTEXT/core_quality.py build \
  --context CONTEXT --source-sha256 EXACT_PARENT_SHA256 \
  --mode full --output NEW_PRIVATE_FULL_OUTPUT
```

The client SHA and rootless socket must match existing qualification. Client
registry auth is empty and private; there is no push/exported runtime image.
The builder pins official Rust1.98.1 Bookworm, Node24.21.0 and Bun1.4.3 images by digest,
Helm3.21.0 and kubectl1.37.2 downloads by SHA. Guest checks run nonrootUID1001,
with Cargo jobs3 and Rust test threads3. Dedicated Bookworm/Bun caches are locked
and shared only by trusted platform source. The existing full-process-sandbox
BuildKit aggregate remains4CPU/8GiB; solves run sequentially. Browser tools are
locked by the Playwright package and its browser revision. There is no Docker
daemon/socket, privileged Pod, VM, global token or host-network exception.

Focused coverage:

- `cargo test --locked -p spitzli-access --lib waitlist`: consent/admission-field
  validation and rejection of C1/control characters before database access.
- `cargo test --locked -p spitzli-control-plane --lib waitlist`: actual local
  HTTP negative cases for JSON/consent/admission fields, origin and request size.
- `cargo test --manifest-path plugins/lumaserv-compute/Cargo.toml --locked
  --test process`: real plugin child process/private RPC, locally generated
  provider HTTPS, scoped authorization/redaction/revocation and exact full panel
  equality against the Console consumer fixture. No real provider credentials.

Required regression names and nonzero passed-test counts are checked. A zero
filtered run cannot qualify. If the source test implements the optional
`SPITZLI_COMPUTE_PANEL_EVIDENCE` export, the exported actual output is independently
compared to the consumer fixture; absence is recorded without inventing output.
The independently published sibling compute repository requires a separate
frozen source archive for separate-repository proof; root closure does not
silently establish that sibling's current source identity.

Full coverage runs the checked-in quality checker regressions, canonical
`bun run quality` and exact requested root `npm run build`. That includes source/dependency
boundaries, lint inheritance, strict Serde, frozen migrations, workspace and
standalone Rustfmt/Clippy policy across feature variants, library and local RPC/
plugin/provider regressions, Console lint/type/unit/build/all browser fixtures,
full Cargo workspace build and realtime bundle. Helm render and actual local
Core probe-routing tests follow. Source-supplied ignored live/privileged tests
remain ignored; this gate does not establish real-account/provider parity.

Official Docker29.9.2/Compose5.6.1 clients have been qualified in the approved
EU sandbox using only a disposable `config` rendering, an explicitly nonexistent
daemon endpoint and no socket. Receipt: `core-tooling-qualification-2026-11-01.json`.
Full mode now also runs the actual native `check-compose.py` gate after GO; that
full-source gate is not implied by the separate tool-only qualification. Production Dockerfiles were identified
at `deploy/docker/backend.Dockerfile` and `deploy/docker/Dockerfile`; they are
packaging recipes using Trixie, not this whole-source quality recipe. The older
agent-only safe source selector omits the frontend/plugins/tools/quality closure
and is not reused as an incomplete global-quality source input.

Successful outputs contain exact source/harness pins, executed command ledger,
exit codes, test counts, complete per-check logs and compiler/package provenance.
Every nonzero command fails the solve. Build failure output retains a private
bounded2MiB diagnostic tail on BUILD; stdout prints only its path. The helper
never promotes, publishes or deploys an artifact. Failures are reported for
parent review rather than changing platform source or weakening checks.

```sh
python3 -m unittest discover -s tools/plugin-ci -p test_core_quality.py
```

These synthetic tests validate archive isolation, ambient-state rejection,
fixed locked focused plans and rejection of empty/filtered/missing-regression
success. They do not compile or test the working tree.

## Additional separately selected gates

`--mode consumer` runs only the frozen compute UI contract test after a locked
Console-filtered Bun install. This parses the same Golden JSON compared by the
actual Rust subprocess response against the actual Console schema. It never
fabricates a backend-output export.

The separately prepared `--mode website` and `--mode migration` targets require
the parent's final source freeze/GO. They use official PostgreSQL18 Bookworm
pinned by verified registry index in `core-postgres-fixture-base.json`, within
the same qualified rootless BuildKit. No Docker daemon or supplied database/CMS
URL is accepted. A fresh nonroot cluster has its own private data/socket root,
random fixture credentials, loopback-only TCP,20 connections and32MiB buffers.
The driver verifies the actual data directory and stops its server before
removing the owned temporary tree, including every failure path.

Website acceptance runs actual Payload schema migrations and seeds the authored
landing defaults using the local API into an empty owned database. Schema
push=false is retained. It then runs the actual production Next build and
production server, executing `apps/website/tests/site.test.mjs` only against its
own loopback origin. There is no mocked CMS response or live CMS/database URL.
The current installed Next CLI guide was read before choosing build/start flags.

The0073 acceptance gate applies actual0068 and0073 source SQL, with explicit
minimal owned prerequisite roles/rate-limit/invalidation fixtures. It snapshots
all preexisting native/imported/reviewed rows and ownership/RLS/column privileges,
then proves they stay unchanged. Actual future public intake receives the new
2026-11-01 notice default; repeated intake does not replace an old record. This
is focused migration semantics acceptance, not a full Core schema upgrade or
production migration. Frozen0068 bytes are never changed.

Source-only tests for these guards:

```sh
python3 -m unittest discover -s tools/plugin-ci -p 'test_core*.py'
```

## Runtime identity and final source hygiene

Node24.21.0 is copied from its verified official Bookworm image into both Rust
and PostgreSQL stages. Before application checks the helper requires actual
`process.release.name=node`, exact version, expected executable and no Bun
version field, plus the binary SHA observed during the official-image tool
qualification. Docker/Compose binaries and exact versions are likewise checked
for full mode. No application script is changed to accommodate tooling.

Source preflight rejects `core`, `core.<pid>` and compiled public/agent ZIP
patterns. Final snapshots omit those preexisting tracked outputs without changing
original repository files or index. Only the three legitimate `crates/secrets`
source files are permitted; actual private/runtime/environment files stay out.
The deliberately vendored upstream startup TGZ remains an explicit hash-pinned
source input. A marker scan is a limited packaging check, not a statement that
preexisting core dumps contain no sensitive information.

## Next-freeze native PostgreSQL and recovery gates

`native-pg` is prepared only for a fresh reviewed source snapshot containing the
policy worker's opt-in local fixture. The Rust Bookworm build runs the exact
pure retirement policy unit test, compiles plugin-host `resource_admission`, and
compiles control-plane lib/bins/tests without running them. Required main, plugin
controller/control and agent-control bins must be present in Cargo artifact
messages. Recovery-mail support is compiled through the exact control-plane
`bootstrap` test target, with matching package ID, manifest and source path. Its
unconditional bootstrap→plugin_builtins→recovery_admission→recovery_mail_component
module chain is recorded with source hashes; support filenames are not targets. The selected integration binary is hashed
and copied into the official PG18 Bookworm runtime. No Cargo tools or Cargo
cache are needed there. Fixed PG tools include `createdb`.

The runtime executes exactly
`retired_postgres_stale_grants_cannot_create_but_retained_work_and_maintenance_survive`
with `--ignored --exact --nocapture --test-threads=1` and
`SPITZLI_PLUGIN_TEST_LOCAL_POSTGRES=1`. Its test-only adapter creates and owns
its own private cluster/database and accepts no supplied DSN. ELF/library proof,
exact1-test success and disappearance of owned fixture roots are required. The
harness does not enable arbitrary ignored/live tests or modify production roles.

`recovery` is prepared for the owner-only0074 tool's actual PostgreSQL/fake-provider
acceptance. The outer driver creates a fresh cluster and generated `_test` DB
owned by fixture-only login `neondb_owner`, then invokes the exact opt-in test
with `SPITZLI_TEST_NEON_RECOVERY=true`. The supplied URL is generated internally
for that loopback cluster; no external database/provider URL or credential is
accepted. The recovery login starts NOSUPERUSER, NOBYPASSRLS, NOINHERIT, NOCREATEDB and
NOCREATEROLE, owning no database or objects. A separate generated bootstrap
login owns the fresh DB and creates actual FORCE-RLS/column-grant fixtures; the
test grants only explicit SET membership after initial denial checks. The actual test owns its local
HTTP fake provider and checks dropped response recovery, audit rollback, replay,
retained credentials/MFA/passkeys/grants and role denial. The outer driver stops
and removes the cluster after the test.

These additions have not executed against the old `c9` snapshot. Full root
quality remains failed there on the original archives lock. The separately
validated candidate changes only the existing identity->reqwest dependency edge;
parent must apply it and coordinate all source changes before another freeze.

The next focused plan additionally runs
`cargo test --locked -p spitzli-build --lib
repository_projection_exposes_the_authoritative_binding_not_metadata_ids` with
default service features enabled. It proves returned repository identity comes
from the authoritative binding rather than spoofable metadata. The original
waitlist/compute/consumer coverage remains in the next plan; no mutable-source
check has been executed before the parent-reviewed freeze.

For the next freeze, npm11.19.0 and its bundled module tree are copied from the
SAME official Node24.21.0 image into both relevant stages. Symlinks point to that
exact CLI; version, real path and CLI SHA are verified and recorded. Only root
`npm run build` executes the existing Cargo+Bun script. Installs stay frozen Bun;
there is no npm install, package-lock generation or duplicate equivalent root
build beyond canonical quality's own checks. Tool-only npm qualification is
recorded in `core-npm-tooling-qualification-2026-11-01.json`.

Restricted recovery passes both internally generated URLs for the same owned
loopback endpoint/database: neondb_owner and separate quality bootstrap. It
asserts recovery-role flags and database ownership before/after, never supplies
a live URL, and never grants membership itself. The exact acceptance test
performs bootstrap setup and the reviewed SET-only membership transition.

Full mode also verifies the exact compiled German catalog with the checked-in
GNU gettext `compile.py --check`; gettext tools are explicit native prerequisites.
The freeze inventory records the20 PO files and4321 compiled entries. Restricted
recovery now starts with no superuser/BYPASSRLS/INHERIT/role-creation powers and a
separate bootstrap-owned DB; the earlier superuser fixture proposal is superseded.

The next prepared context can carry ONLY the reviewed r3 control-plane compile:
original source/manifest/proof/assessment/log hashes are pinned, all66 local
artifact dep-info mappings exclude the changed local PG fixture, and source
comparison rejects any changed CP/library/lock/compiler/other-fixture input.
Records retain original r3 execution and `reexecuted=false`. Resource-admission
is always rebuilt and its exact live test always runs fresh. Without this
explicit validated checkpoint the original compile commands remain unchanged.
Full canonical quality never uses this extra compile carry.

Recovery dependency closure uses the existing full frozen Bun lock with scripts
ignored. Its exact pg/zod/recovery-helper import preflight runs in the recovery
stage before the PG driver can create a cluster. The prior database-runtime-only
filter omitted zod and is no longer used. The full canonical quality/root build
stages are unchanged. Native PG's actual r4 pass and affected9 original552
execution remain separate carried evidence; no CP-carry checkpoint is needed
when resuming directly at recovery.
