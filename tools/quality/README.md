# Native quality checks

Run `bun run quality` from the repository with Bun on `PATH`, the pinned Rust toolchain, `protoc`, and Playwright Chromium installed. This is a check-only gate: it does not format files, change database schemas, publish artifacts or deploy anything. The runner does not explicitly load production environment files; subprocesses inherit the shell environment and retain their normal tool behavior, including Vite's environment-file handling. Use a clean, isolated checkout without production configuration. Every subprocess failure fails the gate.

The checks are also available separately:

```sh
bun run quality:contracts
bun run quality:rust
bun run quality:frontend
```

The Rust binary accepts `contracts`, `rust`, `frontend` or `all`, followed by an optional repository root. Relative roots are resolved before constructing subprocess paths. It uses argument arrays rather than shell interpolation.

## Rust policy

`Cargo.toml` defines the canonical Rust/Clippy policy; every crate inherits it. Independent provider workspaces and this checker carry exactly the same table, enforced automatically. Ordinary correctness, suspicious, complexity, performance and style diagnostics are errors, along with selected rules for unsafe documentation, held locks across await, checked conversions, unused futures, fallible conversions, redundant control flow, unfinished implementation macros and unnecessary indirection. Suppressions need local reasons; fulfilled expectations are checked. Generated Tonic code has narrow, reasoned exceptions for generator output.

`clippy.toml` caps nesting at six levels. Prefer small functions, guard clauses and exhaustive enums/matches; the ceiling is not a target. Rustfmt is mandatory for the workspace, provider workspaces and checker.

The gate checks all targets with all features and without default features. It also compiles the agent with neither adapter, Docker only and Kubernetes only, and the Build worker without service features. These isolated checks catch failures hidden by workspace feature unification. It runs library tests, agent protocol and supervisor journal contracts, and isolated daemon management/outbox/configuration regressions with runtime adapters disabled. These exercise exact callback replay and optional-domain compatibility through controlled local RPC peers. Pure Docker/Helm manifest policy tests include both adapters without connecting to either runtime. Plugin contract/transport tests and each standalone provider's locally isolated tests also run. Database, Docker/Kind, VM and real-account acceptance fixtures remain separate explicitly controlled checks; a green quality gate does not establish product parity.

Broad `pedantic`, `restriction` and `nursery` sets are not enabled indiscriminately. A separate survey found remaining potential truncation62, wrapping55, sign-loss37, precision-loss10 and large-future33 source sites. These are candidates for semantic review, not proven defects or implicitly approved casts. Those rules have not been silently suppressed. Do not infer that every domain API is already a typed DTO: generic plugin/wire payloads legitimately use JSON, and remaining raw JSON domain interfaces still need migration work.

## Project-specific rules

The checker uses maintained `cargo_metadata`, TOML and Syn parsers.

- **Dependency boundaries:** provider implementations and their known vendor SDK stay outside platform authority/domain implementations. Providers may use shared protocol, message, runtime and HTTP libraries; those shared libraries cannot pull in authority/domain implementations. Normal and build dependency edges include renamed, optional and target-specific local dependencies. Dev-only test edges are excluded. All standalone plugin manifests under `plugins/*/Cargo.toml` are discovered. This checks Cargo dependencies, not arbitrary code copied into another crate or the behavior of build scripts. The explicit shared set includes `spitzli-export-protocol` (typed source/lease RPC data) and `spitzli-object-storage` (verified byte transport without product authority). They remain subject to the shared-to-domain/provider prohibition; a real Cargo metadata regression checks both allowed consumption and a forbidden transitive domain edge.
- **Policy inheritance:** new workspace crates cannot omit the shared policy, and standalone lint tables cannot drift from it.
- **Strict Serde commands:** an internally tagged `Deserialize` enum with `deny_unknown_fields` must use empty struct variants instead of unit variants. Serde otherwise accepts unexpected fields on those unit commands. The rule reports file/line and is covered by syntax tests and real deserialization regressions. Serialized JSON stays unchanged when converting such variants to `{}`.
- **Frozen migrations:** `crates/persistence/migrations/frozen.json` records the reviewed SHA-256 of native migrations already accepted against the retained local fixture. Changed or removed files fail, including changed targets behind the existing domain symlinks. Targets outside this repository and malformed/duplicate manifest entries are rejected. Add a new migration for subsequent schema changes. New files may remain unfrozen during implementation; append their checksums only after migration acceptance and review. Changes to the checksum history itself require review; this check complements the database's migration journal and does not establish production rollout.

The Serde rule recognizes direct/qualified/module-level imported aliases and recursively examines conditional attributes. It conservatively treats conditional attributes as potentially active; it does not evaluate arbitrary cfg predicates, expand macros or resolve aliases through other crates. Mutually exclusive conditional schemas can therefore require review. Prefer explicit stable wire contracts rather than conditional serialization shapes.

## Frontend and CI

The Console uses type-aware strict/stylistic TypeScript ESLint presets, React Hooks checks, no inline suppression or TypeScript escape comments, exhaustive switches and bounded depth/complexity. Its compiler additionally checks indexed access, exact optional properties, returns, overrides, unused declarations and casing. See `apps/console/README.md` for details. The gate runs lint, typecheck, unit tests, production build and the local browser fixture.

`.github/workflows/quality.yml` runs public platform-source checks on same-repository pushes and explicit manual dispatch. It selects only the documented `blacksmith-4vcpu-ubuntu-2404` runner, with an operator qualification gate and recorded provider region evaluated before allocation. There is no hosted or former self-hosted fallback. The Blacksmith app is installed and US West configured, but actual execution is unqualified, so normal CI remains blocked; skipped jobs do not establish release readiness. See [Blacksmith prerequisites](../plugin-ci/BLACKSMITH.md). No tenant or deployment credentials enter this read-only job, and checkout credentials are not persisted. Customer image builds remain a separate EU K3s workstream. The workflow installs the pinned Bun prerequisites, verified Helm and locked Playwright browser with OS dependencies. GitHub branch protection remains separate repository administration.

Run checker regressions directly with:

```sh
cargo test --manifest-path tools/quality/Cargo.toml --locked
```

Native deployment checks run separately with `bun run test:distribution` and in CI. They require
Docker Compose, Helm and Python with PyYAML. They render disposable configurations and send the
rendered Helm probe host/path through the actual Core router; no cluster or database is contacted.
