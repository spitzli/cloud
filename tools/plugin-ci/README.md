# Current GitHub CI policy: Blacksmith only — execution blocked

The owner superseded the persistent runner choice on 2026-11-01. Follow
[Blacksmith migration and release blockers](BLACKSMITH.md) for current CI.
The new source selects only Blacksmith with pre-allocation provider/builder gates;
the app is installed and US West configured, but actual execution and managed
builder qualification proof remain pending. The first maintainer probe is
restricted to the canonical repository and its exact qualification branch;
normal readiness flags stay unset.
Skipped workflows do not mean CI passed or a release is main-ready.

The material below records the **historical Contabo qualification**. Its setup,
registration and selection commands are not the current migration procedure.
Do not run new/rerun GitHub jobs there. Retiring the former runner and its
credentials/cache remains a separate operator action after replacement checks;
no retirement was performed in this source change. Customer build infrastructure
must be preserved.

# Historical trusted persistent platform CI — verified EU BUILD host

The owner selected a **persistent trusted-maintainer runner**, not an ephemeral
or customer-code sandbox. It is installed on the existing Contabo EU2 BUILD host
`167.86.75.144`, instance `e6d9884c-e6bd-4699-8a0c-99f2358ae8a1`, using existing
`spitzli-plugin-ci` UID/GID `1001`. Location comes from the owner's verified
provider inventory; labels and this script do not independently prove geography.
No VM, privileged pod, Docker daemon, tenant namespace or customer build
admission was created or changed.

Actual GitHub runner: `cloud-plugin-eu2-trusted-01`, runner ID **39**, version
**2.338.2**, org group `cloud-plugin-trusted-eu2`, group ID **3**. Group visibility
is **selected**, with exactly the nine `cloud-plugin-*` native repositories and
`cloud-plugin-sdk` initially admitted after their published workflow gates were
checked. CMS is added separately only after its own workflow is reviewed. The
runner accepts one job at a time. Repository Actions enabling/triggering remains
an explicit operator action; `manage_runner.py` never enables repository Actions.

Runner labels for `WEBDOCK_PLUGIN_CI_RUNNER`:

```json
["self-hosted","Linux","X64","spitzli-plugin-eu2","trusted-platform"]
```

A routine allowlist refresh preserves an already-reviewed CMS repository; the
explicit `--include-cms` flag is required only for its initial admission. If the
owner later expands the group to the separately reviewed legacy repositories,
this initial native/CMS selection script refuses replacement rather than
silently dropping those repository IDs; use the owner-managed union policy.

The group excludes all other repositories, including private job-demo and
withdrawn managed-registry. Every admitted workflow excludes fork PRs before a
runner is allocated; release jobs require a maintainer push to a release ref.
No `pull_request_target`, issue-comment or repository-dispatch entry point is
accepted by the source-gate checker. Same-repository source is trusted platform
source; this setup makes no hostile-customer isolation claim.

## Reproducible setup and qualification

`qualified_host.py install` is copied to the qualified host and run as its
existing root SSH identity. It verifies the existing non-root BuildKit user and
client pin, installs distro build/protoc/GitHub CLI dependencies without package
upgrades or service restarts, extracts Rust **1.98.1** from the already-pulled
immutable official image, and installs checksum-pinned official Bun **1.4.3**
and Actions runner **2.338.2** archives. Their URLs/SHA-256 values are in source.
The image compiler remains pinned; requested rustfmt/clippy components are
installed through the same trusted rustup for exact toolchain 1.98.1, never a
mutable stable/latest channel. Rust/Cargo/rustfmt/clippy/Bun/protoc/gh/buildctl
versions are checked under UID 1001. The service and UID-specific login profile
set RUSTUP_HOME=/opt/spitzli-plugin-ci/rustup, writable CARGO_HOME under HOME,
CARGO_BUILD_JOBS=3 and CARGO_TARGET_DIR=/home/spitzli-plugin-ci/native-validator-target.
Actual Listener process environment was checked against those qualification values.

Server SSH uses the authorized local `~/.ssh/id_ed25519` explicitly with
`IdentityAgent=none`; no OnePassword agent is needed for that server connection.
This does not change the operator's separately registered Git signing identity.

`manage_runner.py register` uses the operator's local `gh` authentication to
obtain a short-lived org registration token and transfers only that token over
SSH stdin. Registration supplies it through the runner's input environment,
never command arguments or logs. No PAT, OnePassword key or authenticated `gh`
configuration is copied to BUILD. Persistent runner credentials are mode 0600;
its generated `.env` contains only `LANG`. Auto-update is disabled to keep the
reviewed archive version fixed; the owner must review and refresh the pinned
runner before GitHub rejects an outdated version.

```sh
python3 -m unittest discover -s tools/plugin-ci -p 'test_*.py'
python3 tools/plugin-ci/manage_runner.py verify
# Requires initial new-repository Actions still disabled and exact source gates.
python3 tools/plugin-ci/manage_runner.py select
# Only after CMS source/variables/workflow are ready; newly admitted CMS stays disabled.
python3 tools/plugin-ci/manage_runner.py select --include-cms
```

Owned tmpfiles configuration creates only `/run/user/1001` mode0700 at boot.
The enabled runtime-preparation/egress units precede the enabled BuildKit unit;
the runner requires all three. `systemd-analyze verify`, enabled-state and actual
dependency-property checks passed without rebooting the server. Operation needs
no workstation tunnel or an online personal computer.

The service `spitzli-plugin-runner.service` runs the official `bin/runsvc.sh` as
UID 1001, capped at 3 GiB, 2 CPUs and 512 tasks. It requires the separate existing
rootless BuildKit service and the CI egress guard. The qualified BuildKit
v0.34.1 client is `/opt/spitzli-plugin-ci/buildkit-v0.34.1/bin/buildctl`, SHA-256
`8e64362dceba989827b23af567915feda10426ebd40611772318662e33bf2bfd`, socket
`unix:///run/user/1001/spitzli-buildkit.sock`. Its full process sandbox and overlayfs
worker were tested by the owner with a real Debian RUN plus OCI export. BuildKit
has separate 8 GiB/4 CPU/1024-task bounds. No Docker socket/daemon is used.

Persistent state includes runner credentials, workspaces, compiled Cargo/Bun
caches, BuildKit layers, diagnostic logs and private redacted failure logs.
Trusted jobs share this UID and can affect later trusted jobs. Removing one job's
auth file does not erase that persistent state. No global registration credential
is retained; re-registration needs a fresh short-lived token from the operator.

## UID egress policy

`egress.py` owns only nftables table **`inet spitzli_plugin_ci`**, with one host
OUTPUT chain matching `meta skuid 1001`. It does not flush or modify other tables,
INPUT/FORWARD policy, K3s resources, CNI or customer namespace policies. The
persisted `spitzli-plugin-ci-egress.service` applies it before the runner starts.

The policy rejects private/loopback/link-local/metadata IPv4/IPv6 destinations
and observed BUILD/RUN platform addresses, including SSH, database and Kubernetes
API ports. Exact host stub DNS `127.0.0.53:53` UDP/TCP remains allowed. The owner
explicitly authorized **`79.143.177.157:443`** for registry release publication.
That address is the shared RUN HTTPS ingress: the exception also permits other
HTTPS virtual hosts at that address. This is **not full RUN network isolation**.
Registry requests still require TLS, operator-pinned JWK verification and a
scoped publisher token; those secrets are unavailable to fork jobs.

`verify_network.py` runs nonsecret connection probes as UID 1001 and checks nft
counter deltas. On 2026-11-01 it proved rejection of loopback/BUILD SSH, private
Kube API, RUN database, IPv4 metadata and IPv6 loopback/BUILD SSH, while both stub
DNS transports, public GitHub HTTPS and registry HTTPS succeeded. Root SSH stayed
reachable. IPv4 admin-prohibited produces errno 113, while IPv6 produces errno
13; counters independently establish the deny rule was hit.

Registry/platform address changes require review and a narrow source update;
there is no wildcard private-network or shared-ingress bypass. Existing
customer public-build admission stays closed.

## Reviewed legacy source and retired import backend

The owner subsequently added the four existing public v1 repositories by an
explicit selected-ID union, preserving the original eleven. All four actual EU
checks passed, with candidate publication skipped and current v1 release bytes
unchanged. The old private GitHub-hosted arbitrary-source workflow was read,
then disabled together with repository Actions after confirming no active runs.
It was never added to this group or shared cache. See
[the evidence and retirement note](LEGACY-CI-AND-SOURCE-IMPORT-RETIREMENT.md).
