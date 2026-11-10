# Isolated build worker

This worker is **disabled until provisioned and independently verified**. Unit tests use
fake commands; the opt-in local KVM acceptance below executes real disposable guests.
Neither establishes EU placement, registry permissions, or a successful Vercel
deployment. Never run the QEMU build mode on a production application/control node.
The separate artifact-only mode below performs no customer code execution and may
run as an unprivileged service on an existing application node.

EU placement is preferred, not an enrollment requirement. Record the actual uppercase
two-letter country code. For an unknown location use `ZZ` and explicitly include
"unverified" in the location evidence; never claim EU verification without evidence.
Isolation, credential, storage and image verification remain required.

## Runtime

`python3 worker.py --config /etc/webdock-build-worker/config.json [--once]` leases builds
and releases over outbound HTTPS. Concurrency is one, guarded by a filesystem lock.
Configuration and image are root-owned and not group/world writable; run the worker as
an unprivileged `webdock-builder` user with KVM access. The credential file may be 0640,
root:webdock-builder. The artifact root must be an existing private 0700 directory.

Configuration fields:

```json
{
  "controlURL": "https://auth.example/api/hosting/git/worker",
  "credentialFile": "/etc/webdock-build-worker/credential",
  "country": "DE",
  "locationEvidence": "operator verification record reference",
  "storageEvidence": "storage location and backup verification record reference",
  "isolation": "qemu-kvm",
  "isolationEvidence": "independent isolation test record reference",
  "image": "/var/lib/webdock-build-worker/base.raw",
  "imageSHA256": "REPLACE_WITH_VERIFIED_64_HEX_DIGEST",
  "artifactRoot": "/var/lib/webdock-build-worker/artifacts",
  "egress": "loopback-connect-proxy",
  "egressEvidence": "proxy allowlist and private-network rejection test record",
  "registryProjects": {
    "123": {
      "repository": "registry.example/customers/456/projects/123",
      "authFile": "/etc/webdock-build-worker/registry-project-123.json"
    }
  }
}
```

Evidence strings reference actual administrator records, not self-attestation generated
by this program. Worker enrollment must separately verify them. `validate_config`
checks a pinned root-owned raw image and `/dev/kvm`; it does not independently verify a
datacenter address.

## Build a disposable guest image

Prepare a Linux VM with Node 24, npm, Python 3, pinned Vercel CLI **63.1.2**, BuildKit,
runc, systemd-networkd or DHCP networking, and virtio block/console kernel support.
The official [CLI 63.1.2 manifest](https://registry.npmjs.org/vercel/63.1.2)
pins `@vercel/build-utils` 14.20.1, whose published runtime table includes Node 24
without an experimental environment flag. `install-guest.sh` checks the actual installed
build-utils runtime selector for `24.x`, as well as the CLI version. Inside that
VM run `install-guest.sh` from this directory. Shut down, remove cloud-init and machine
credentials, convert its disk with `qemu-img convert -O raw`, and record its SHA-256.
Use a fixed-size disk small enough for the aggregate build allowance. Admission reserves two bounded QEMU files, archive staging copies and filesystem-entry overhead; oversized images fail before boot. The default 10GiB job allowance permits roughly a 3GiB raw image. Check its
installed dependencies and CLI pin in the same image verification record. The supplied
systemd guest service executes the actual Dockerfile/production Vercel build and powers
off; no external runner implementation is required.

QEMU runs with KVM, CPU/memory limits, a disposable snapshot overlay, no host shares,
no monitor, and a bounded timeout. Host input is a tar on a read-only virtio block device; the only result channel
is a virtio serial file. Output growth and child processes are bounded. The per-file QEMU ceiling and upfront aggregate reservation cover both snapshot and malicious result-channel growth; free disk space is checked before boot. Snapshot
files live in a fresh temporary directory and are removed after every attempt. There
is no host Docker socket. VM jobs never contain enrollment, GitHub, registry, Vercel,
or Kubernetes credentials. Build values are explicitly authorized and treated as
readable by customer code.

With egress configured, QEMU user networking has `restrict=on`; only guest forwarding
to the loopback CONNECT proxy is enabled. Run `egress_proxy.py --config <root-owned-json>`
as a separate unprivileged service. Its config is `{"allowedHosts":["registry.npmjs.org"]}`;
add the exact required package/image hosts after review. It permits only TLS CONNECT
port 443, validates every DNS answer is public, and connects to the checked address.
No wildcard hosts or arbitrary port forwarding. Without verified egress, VM networking
is disabled and dependencies/base images must already be available. The supplied BuildKit guest service sets
the proxy environment for remote image pulls; Dockerfile
RUN steps receive the same proxy via automatic proxy build arguments. Never allow the
control plane, artifact credentials, metadata endpoints, or internal registries here.

Vercel recipes currently require `package-lock.json` and run `npm ci`. The trusted
service provides target settings and explicitly permitted build values. The VM builds
with `vercel build --prod`; it never runs `vercel pull`. Dockerfile authors control their
own dependency installation inside the disposable guest. CPU <=8, memory <=16GiB,
duration <=30 minutes, disk <=32GiB, input <=128MiB, artifact <=512MiB, logs <=24KB.
Archive paths, links, sparse files, devices, duplicate members and expansion are checked.
Source download accepts a trusted descriptor for codeload.github.com only, fetches
without enrollment Authorization, bounds gzip expansion and removes the wrapper folder.

## Trusted publication

`publisher.py` validates tenant/build storage keys and hashes before intake. OCI
publication validates every manifest/config/layer digest and uses operator-installed
`skopeo copy --preserve-digests`; a project-scoped authfile stays outside the VM.
Registry returned identity is `repository@sha256:...`, distinct from the artifact tar
checksum. Publication is idempotently tagged by release ID and must be reconciled after
any ambiguous write.

Vercel publication runs the same pinned CLI against a newly created workspace containing
only `.vercel/output`, trusted target IDs and trusted `vercel.json`. No repository hooks,
package metadata or executable `vercel.ts` are copied. Credentials live in a private CLI
global auth directory and never appear in command arguments. It only invokes
`deploy --prebuilt --prod --no-wait`. Every function must have Frankfurt
region metadata and a supported non-edge runtime. A submitted deployment is **not ready**:
the control plane must observe actual readiness/regions before marking the release ready. Production publication applies provider aliases.
Errors after publication enter `needs-reconciliation`, never blind retries.

## Agent pulls

The hosting agent reads optional root-owned `SPITZLI_REGISTRY_CONFIG`:

```json
{"version":1,"euStorageEvidence":"verified record","projects":{"123":{
"repository":"registry.example/customers/456/projects/123",
"pullSecret":"registry-project-123","readOnly":true,
"credentialScopeEvidence":"repository-only read robot verified"}}}
```

An administrator installs that imagePullSecret in the exact project namespace and
validation namespace. The agent allows only that exact repository plus immutable digest,
then uses `IfNotPresent`. Credentials are neither environment values nor packet fields.
Other projects retain `Never`. Existing restricted security, Recreate strategy, ownership
and quota rules stay active. BYOK clusters require their own installed policy and secret.

## Checks

`python3 -m unittest discover -s apps/build-worker -v`

`python3 -m unittest discover -s agents/kubernetes -p 'test_registry.py' -v`

Before activation run two hostile builds in a disposable real VM, prove no control
credential or previous build files are visible, verify timeout/disk/log limits and denied
network destinations, then exercise publish/pull/readiness/rollback and Vercel Frankfurt
observation in disposable targets. The worker alternates builds and approved publications so a push backlog cannot starve releases. At startup and hourly it requests the trusted retention list, unlinks only exact approved artifact files using no-follow directory descriptors, and acknowledges removal. Missing files are acknowledged; directories and symlinks are never deleted. Each cleanup also offers at most 1000 generated artifact keys older than 24 hours for server-side orphan verification; it deletes only the returned subset, protecting files with recorded artifacts or active build leases. Retention decisions stay in the control service; customer jobs cannot request deletion. Verify operational free-space monitoring before activation.

Provider references: [prebuilt deployments](https://vercel.com/docs/cli/deploy),
[global auth configuration](https://vercel.com/docs/project-configuration/global-configuration),
[Build Output API](https://vercel.com/docs/build-output-api).

### Opt-in real local VM acceptance

`e2e_real_vm.py` runs the actual `execute_vm` path, QEMU/KVM, guest Python and
BuildKit against authored Dockerfiles. It creates an OCI HTTP app running as UID
65532, then checks a fresh VM has no previous BuildKit cache marker, an authored
failure preserves diagnostics, log flooding is bounded, and a long-running build
is stopped by the guest deadline. A separate five-second host watchdog kills an
actual QEMU process and verifies no child remains. It also checks that the pinned base image hash
is unchanged and temporary execution files are removed. No host Docker build,
control service, enrollment credential or provider publication is used.

Prepare a disposable raw image with the worker/guest scripts and service from the
same checkout, Python 3, BuildKit and runc, plus virtio block/console support. The
Dockerfile-only test image does not need Vercel or Node. Ubuntu minimal images may
require the matching `linux-modules-extra-$(uname -r)` package for their guest
features. Remove cloud-init and machine credentials as above. Obtain a static
BusyBox binary from a trusted distribution package; the script copies it into the
fixture and executes it only inside the VM. Use a private scratch directory on a
filesystem with at least 24 GiB free (a small `/tmp` tmpfs will fail admission).

```sh
python3 apps/build-worker/e2e_real_vm.py \
  --allow-disposable-local-vm \
  --image /absolute/path/to/test-base.raw \
  --image-sha256 VERIFIED_IMAGE_SHA256 \
  --busybox /absolute/path/to/static/busybox \
  --scratch /absolute/path/to/new-private-run-directory
```

The scratch path must not already exist. `evidence.json` records actual build logs,
durations and OCI identities; `http-app/image.tar` is the immutable output to pass
to separate registry/pull/readiness acceptance. This local test verifies offline
network denial, not the optional dependency proxy or datacenter location. It does
not publish to Vercel or verify a real GitHub installation.

Input uses a **read-only virtio block disk** with serial `webdock-input`, and the
root disk has an explicit boot index. Bulk fw_cfg sysfs reads proved prohibitively
slow on real KVM. The guest loops over short virtio-serial writes; a single raw
write can accept only 32 KiB and silently truncate a larger artifact.

## GitHub Actions artifact-only importer and publisher

Set `isolation` to `artifact-only` and enroll the matching artifact-only worker role.
Use `webdock-artifact-publisher.service` with a dedicated `webdock-publisher` user,
root-owned configuration/credential, private `/var/lib/webdock-artifact-publisher/artifacts`,
and the same pinned Vercel CLI 63.1.2 and skopeo required for publication. No KVM,
VM image, Docker socket, build tools or dependency proxy is required. Configuration
still records `country`, `locationEvidence`, `isolationEvidence` (describe the
artifact-only boundary), and `storageEvidence`; it must also include `controlURL`,
`credentialFile`, `artifactRoot`, and any operator-owned `registryProjects` policy.
The service uses `/etc/webdock-artifact-publisher/config.json`.

This mode leases only GitHub Actions imports through the existing claim/complete
protocol, alternates imports with approved publication, and retains local artifact
ownership for rollback/retention. Unexpected build providers fail closed without
source download or VM execution. The control service verifies repository access,
workflow identity, successful run attempt 1, exact source commit and configured
artifact name before issuing a short-lived download descriptor. GitHub App and
publisher credentials are never forwarded to artifact storage.

The downloaded ZIP must contain exactly `manifest.json` and `artifact.tar`.
The importer compares the GitHub archive SHA-256/size, then the manifest's
repository, commit, ref, workflow, run ID/attempt and inner TAR SHA-256. ZIP links,
extra/duplicate entries, encrypted files, traversal and oversized members fail closed.
Only GitHub Actions result hosts and `productionresultssa*.blob.core.windows.net`
HTTPS hosts are allowed, without redirects or Authorization headers. ZIP is limited
to 513 MiB, manifest to 64 KiB and TAR to 512 MiB; existing TAR path/type/count limits
apply. Keep at least several GiB of staging space available for nested validation.

Bot TARs contain only `image.tar`, validated recursively as single-platform OCI.
Panel TARs contain `.vercel/output`, `.vercel/project.json`, and trusted
`vercel.json` containing only Frankfurt regions. Panel manifest `vercel` metadata
must match CLI 63.1.2, Node 24, production target, configured source directory and
trusted project/org/settings. Project settings normalize `rootDirectory` to null
because Actions builds inside that source directory. The importer discards the
project configuration and stores only validated Build Output API output. Local
publication recreates its own trusted project configuration. Customer scripts,
package hooks, source configuration and artifact functions are never executed by
this service. The supported Next.js output metadata cannot enable local bundling.


## Source plugin installer runtime contract

`plugin_installer.py` uses `plugin-installer.example.json` for its separate
artifact-only installer configuration. UI plugins require operator-owned
`runtime.consoleOrigin` plus their existing UI ingress/TLS settings. Set the exact
canonical HTTPS Console origin (for example `https://console.example.com`), with
no trailing slash, path, credentials, query, fragment or wildcard. It is validated
and emitted unchanged as `SPITZLI_PLUGIN_CONSOLE_ORIGIN`; it is never inferred
from the API `hostOrigin`. Headless installations may omit Console/UI fields.

The public invocation key remains plaintext PEM in `SPITZLI_PLUGIN_PUBLIC_KEY`;
`SPITZLI_PLUGIN_RUNTIME_TOKEN` remains the independent private runtime credential.
TLS files are mounted at `/runtime-tls/tls.crt` and `/runtime-tls/tls.key`.
Kubernetes starts installed plugins as UID/GID 1000 with a read-only root, even if
the image declares another non-root default. A UI image must be readable by that
UID and expose its public UI separately on 3444, with private actions on TLS 3443.
Upgrade the installer source and add `runtime.consoleOrigin` to its existing
private config before installing a UI plugin; preserve existing keys, registry,
GitHub, namespace and runtime-binding configuration.
