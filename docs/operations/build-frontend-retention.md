# Retained frontend build outputs

`dockerfile-frontend` explicitly retains completed files from the final image
filesystem alongside the original OCI archive. Ordinary `dockerfile` builds keep
their existing exporter and envelope behavior. This path does not deploy static
sites or enable a CDN, cache, migration, TTL, or artifact deletion policy.

At the selected repository `rootDirectory`, commit `.webdock/frontend.json`:

```json
{"version":1,"directory":"app/dist"}
```

The directory is relative to the final image filesystem root, independently of
the repository root. Config is limited to 4096 bytes, requires version 1, and
rejects extra/duplicate fields and absolute, empty, dot, parent, and backslash
paths. The guest packs regular files beneath the selected directory into
`frontend.tar`, with relative names and normalized metadata. It rejects symbolic
and hard links, special files, invalid paths, empty output, and more than 10,000
entries, counting directories too. The full encoded retained envelope, including
OCI, frontend, source, evidence and signature, shares the 512 MiB artifact ceiling.
The source ceiling remains 128 MiB; the source SPDX inventory remains limited to
64 MiB and describes source-file hashes, not resolved OS or dependency packages.

## Worker and reader compatibility

The protected worker config must explicitly set `frontendOutput: true` only for a
new immutable VM containing the frontend-capable guest. Keep the image, kernel,
and initrd hashes independently pinned. Omitting this flag retains Dockerfile-only
claims even with a new worker binary. The private capabilities endpoint must
advertise format 2 and the new recipe before that worker requests a frontend lease.
Old HTTP build callers default to `dockerfile`; recipe filtering occurs before
lease assignment. Ordinary remote/signing workers retain format-1 preflight.

Format 0 remains `<sha256>.tar`, format 1 remains `<sha256>.e1.tar`, and frontend
format 2 uses `<sha256>.e2.tar`. Old HTTP release callers default to format 0.
Readers explicitly advertise supported envelope versions; SQL filters them
before publication leases. Unknown formats and suffix/content mismatches fail
closed on local load and remote hydration.

The trusted worker constructs provenance from the authorized build, normalized
source, exact frontend/configuration hashes, and VM pins. Guest-supplied
provenance is rejected. Version-2 Ed25519 signatures have a separate domain from
version 1. Remote receipts, canonical Core completion, project ownership, approval,
registry scope, active leases and rollback checks remain required. S3 upload alone
does not complete a build. Deterministic preparation/limit failures use the durable
FAILED-completion outbox; ambiguous remote transfers remain retry/reconciliation
work.

`webdock_build::frontend::retained` is a private library consumer: callers supply a
Core-authorized exact receipt, expected complete build scope and optional external
signer pin. It validates before returning an in-memory file archive. It accepts no
file path or URL selector and creates no customer-facing download route. Existing
Archives repository export authority and executable-mode semantics are unchanged.

## Owned qualification

Use a new destination for every VM image. Never overwrite the retained
`.runtime-test/build-vm` baseline. The preparation script refuses an existing
destination. It uses checksummed Alpine 3.22.6, signed packages,
`linux-virt=6.12.114-r0`, BuildKit/buildctl `0.21.1-r5`, and runc `1.3.4-r1`.
On hosts without a musl GCC wrapper, the verified local preparation used Clang:

```sh
RUSTUP_TOOLCHAIN=1.94.0 CC_x86_64_unknown_linux_musl=clang \
  AR_x86_64_unknown_linux_musl=llvm-ar \
  CARGO_TARGET_DIR="$PWD/.runtime-test/frontend-target-OWNED" \
  bash crates/build/tools/prepare-local-vm.sh --allow-disposable-local-vm \
  "$PWD/.runtime-test/build-vm-frontend-OWNED"

WEBDOCK_FRONTEND_VM_TEST=1 \
  WEBDOCK_FRONTEND_VM_ROOT="$PWD/.runtime-test/build-vm-frontend-OWNED" \
  cargo +1.94.0 test --offline --locked --release -p webdock-build \
  --no-default-features --test frontend_vm -- --include-ignored --nocapture
```

The guest acceptance requires real KVM and checks exact retained HTML/JS plus
links, FIFO, traversal, missing-output and output-overflow failures. Its disposable
snapshot and scratch files are removed after every invocation. Preparation
containers/images are removed by the exact-name exit trap; the explicitly retained
owned VM image is not automatically deleted.

The native cross-worker test is
`git_frontend_remote_outputs_cross_worker_publication_and_rollback` in the Hosting
Postgres test target. It needs a fresh owned native database, a newly provisioned
restricted user/policy/bucket on the existing MinIO service, a temporary registry,
and coordinated forward migration 0053. **Do not start that bootstrap while
Mail0052 is being wired or qualified.** The baseline and migrations 0046–0052 must
not be rewritten to add this recipe. Migration0053 must admit the recipe in both
the source constraint and `git_release_authority` while preserving all existing
authority predicates.

Evidence and exact custody/cleanup results belong in a dated verification record.
Local qualification does not establish Wasabi/provider compatibility, production
signing custody, production location, or Kubernetes/static-site deployment.

The completed schema53 native qualification is recorded in
[the dated evidence](../verification/2026-10-25-frontend-output-retention.md).
It includes the real frontend run, ordinary Docker/local legacy and E1
preservation, strict repository-pinned Clippy, VM inventory and exact cleanup.

Primary implementation references: [BuildKit v0.21.1 output parsing](https://github.com/moby/buildkit/blob/v0.21.1/cmd/buildctl/build/output.go),
[version-pinned exporter sessions](https://github.com/moby/buildkit/blob/v0.21.1/client/solve.go),
and [Docker multiple exporters](https://docs.docker.com/build/exporters/#multiple-exporters).
