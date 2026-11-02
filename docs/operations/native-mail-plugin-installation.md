# Native managed Mail plugin

Local native installation and compatible update/rollback have been verified with the
real Console, enrolled agent, Stalwart CE, retained mailbox credentials and a local JMAP
draft. See the [dated verification](../verification/2026-10-24-managed-mail-installation.json)
and [managed Mail scope](../superpowers/specs/2026-10-24-managed-mail-installation-design.md).
This does not establish a production rollout or public mail delivery.

Use the supported bootstrap, enrollment, signed catalog and private storage setup in
[Native plugin installation](native-plugin-installation.md). Mail uses that same
installer and version lifecycle. Its runtime metadata database is separate from
platform PostgreSQL; keep its role/password and sealing key across every generation.

## Curated release and private recipe

Publish an actual reviewed Mail OCI digest and exact signed manifest/descriptor. The
checked-in zero-digest development manifest is not an installable release. Mail uses
logical schema1/readable[1,1]; the three physical plugin migrations are a separate
numbering domain. Compatible code-only update/rollback does not undo SQL migrations.

The operator-owned recipe uses existing generic fields:

```json
{
  "storage": true,
  "sealingKey": true,
  "routeDomains": ["mail.example.test", "mail-admin.example.test"],
  "privateConfiguration": {
    "platformTransport": {
      "kind": "turbosmtp",
      "consumerKey": "<private-operator-key>",
      "consumerSecret": "<private-operator-secret>"
    },
    "trustUnverifiedDomains": false,
    "mailboxes": {
      "image": "registry.example.test/stalwart@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "domain": "mail.example.test",
      "imapPort": 993,
      "submissionPort": 465,
      "admin": {
        "origin": "http://management-ingress.internal:80",
        "domain": "mail-admin.example.test",
        "entryPoints": ["mail-management"]
      },
      "issuer": null
    }
  }
}
```

These are placeholders, not a deployable release or working provider credentials.
Keep the real private catalog/configuration files owned and mode0600. The worker
supplies reserved listener/callback identity, generation, invocation key, storage URL/CA
and sealing-key fields; clients must not provide them.

The target must reach its pinned Core callback, private metadata PostgreSQL and the
reviewed private management ingress. The engine image, route domains, cluster capacity,
storage, Traefik entry points and optional certificate issuer must be available on the
selected enrolled cluster. A null issuer uses the plugin's self-signed engine certificate.
Runtime discovery does not validate Turbo credentials or prove delivery connectivity.

For an outgoing-only installation, omit `mailboxes` and exclude `create-mailboxes` and
`manage-incoming` from the release's supported-feature ceiling. The ceiling applies to
operators too. Incoming-capable installations can still serve outgoing-only customers
without creating their engines.

Local tests may use `{"kind":"smtp","host":"127.0.0.1","port":1025}`. Plain SMTP
and `trustUnverifiedDomains:true` are restricted to that loopback development mode.
A catcher must share the runtime's loopback network to receive messages; a separate
container's localhost is not the plugin's localhost. No production policy is relaxed.

## Install, enable and use

Review Mail's signed release, Workloads permission and target in Console Extensions,
then follow its retained installation operation. After authenticated runtime promotion,
configure the supported self-service features for intended customers.

Installing/enabling Mail, creating domains, connecting a relay or outgoing-only use
creates no Stalwart instance. The first explicit mailbox creation or `incoming.activate`
reserves one CE engine for that customer. All customer projects reuse it while mailbox
and domain access remains project-scoped. The installation itself needs no dummy owner.

Retained startup reconciliation waits for ordinary current-runtime authorization.
Candidate Describe/Health remains available to the pinned Core; a candidate must not
modify customer engines or mark them failed while awaiting promotion.

## Update, rollback and suspension

Use the existing Console Versions page: disable/drain, review the signed compatible
release, then update or roll back. Rollback creates a fresh runtime generation. Core
proves previous runtime Pods stopped before starting a replacement. The metadata
identity/password and sealing key remain, while runtime TLS identity and namespace change.
Customer engine unit/namespace, PVC, administrator and mailbox credentials remain stable.

`incoming.deactivate` suspends the customer's incoming engine and retains its volume.
It stays `suspending` until workload revision and observed zero-replica readiness agree.
Activation and runtime restart reconcile saved intent. Deletion/export is separate.

Do not remove legacy project-engine rows to bypass quarantine. They require explicit
operator migration with their original keys and storage preserved. Incoming-reserved
customers' API sends continue to fail closed until the engine-to-Turbo path exists.
Public reception, recipient routing, real relay and Webmail/outbox completion remain
separate product milestones.

## Owned local acceptance source

The tracked helpers below are opt-in sources, not evidence that acceptance has run.
Imports are inert. Existing PostgreSQL acceptance scripts are unchanged. Root freezes
the reviewed source and binaries, writes the private input and authorizes each stage.
No stage automatically runs the next one, retries failed work, starts a retained cluster
without the input's explicit policy, or asks for another interactive approval.

```sh
bun --no-env-file tools/tests/native-plugin-fixture.mjs preflight --input /absolute/private/input.json
bun --no-env-file tools/tests/native-plugin-fixture.mjs artifacts --input /absolute/private/input.json
bun --no-env-file tools/tests/native-plugin-fixture.mjs prepare --input /absolute/private/input.json
bun --no-env-file tools/tests/native-plugin-fixture.mjs import --input /absolute/private/input.json
bun --no-env-file tools/tests/native-plugin-fixture.mjs start --input /absolute/private/input.json
bun --no-env-file apps/console/tests/mail-installation-native.mjs --report /absolute/owned-run/report.json
bun --no-env-file tools/tests/native-plugin-fixture.mjs observe --input /absolute/private/input.json
bun --no-env-file tools/tests/native-plugin-fixture.mjs cleanup --input /absolute/private/input.json
```

The input is an absolute owned single-link regular file, mode0600. Its v1 fields are:

- `version:1`, `runID`: twelve lowercase hexadecimal characters, and `directory`:
  `<repository>/.runtime-test/native-mail-installation/<runID>` (private mode0700).
- `authorizedStages`: the explicit subset of `preflight`, `artifacts`, `prepare`,
  `import`, `start`, `acceptance`, `observe`, `cleanup` currently authorized by root.
  This list alone may change between stages. All other input fields are hash-frozen.
  Cleanup is the exception: it uses the original hash-checked input snapshot and custody,
  skips source/binary revalidation and does not require a successful prior stage. Updating
  plan checkboxes/source hashes cannot prevent cleanup of the originally owned assets.
- `allowStartRetainedK3s`: an explicit boolean. `false` never permits starting a stopped
  node. The helper records its initial identity/state before any start.
- `kubeconfig`: exactly `<repository>/.dev-local/k3s/kubeconfig.yaml`. Context is fixed
  to `webdock-dev`, API to `https://127.0.0.1:56443`, Docker network to
  `webdock-dev_default` and container to `webdock-dev-k3s-1`. The guard reads only
  `current-context` and the filtered public API address; it never dumps/copies kubeconfig.
  Retained-node Docker inspection is filtered before logging: only required ID, state,
  private network IP and mount identity fields are selected, never Env or command.
- `review.sourceFiles`: relative source path → SHA-256. Freeze the complete Mail image
  build inputs, fixture/browser sources, approved chart and product dependencies. Keep
  operational plan-checkbox progress separate from build inputs. Handovers, private
  generated files and env/kubeconfig paths are refused.
- `review.binaries`: executable name → `{path,sha256}` for `webdock-migrate`,
  `webdock-bootstrap-operator`, `webdock-enroll-agent`, `webdock-bootstrap-plugins`,
  `webdock-control-plane`, `webdock-plugin-control`, `webdock-plugin-controller`,
  `webdock-agent-control`, `webdock-agent`, `bun`, `docker-buildx`. Root supplies eleven reviewed public ELF copies;
  the helpers do not compile them. The agent must include the Kubernetes/plugin-runtime
  and Workloads capabilities required by the approved chart.
  Historical nine-binary snapshots remain readable by cleanup; new execution stages
  require the two tools. No user Docker configuration, credentials or contexts are copied.
- `images`: digest-pinned `postgres`, `registry`, `stalwart` image references. No real
  provider credential is required. `ports`: four distinct free loopback host ports
  named `core`, `console`, `account`, `callback`; the callback port is private Docker
  networking only and is not published on the host.

Subprocesses receive a fixed system PATH and a private HOME/TMPDIR plus only generated
fixture environment. They do not inherit ambient database/provider/proxy/loader variables.
Bun children use `--no-env-file`; programmatic Vite uses `envDir:false` and an owned cache.
The private report contains version/kind/run identity, frozen-input digest, completed
stages, paths, public signed artifact descriptors, observed topology and sanitized proof.
Generated passwords, environment, enrollment, keys and acceptance state stay in separate
private files and never enter stdout, errors, screenshots or the report.

`artifacts` builds `plugins/mail/Dockerfile`, starts an independently owned registry,
signs exact `mail-universal.json` manifests for0.1.0/0.2.0 and exports verified OCI archives.
Every owned image tag must be verified absent and that absence recorded before building;
existing tags and daemon uncertainty are refused without adoption or overwrite.
Before any image intent, the helper verifies the installed public Buildx ELF and stages
only that reviewed file under the private HOME's `.docker/cli-plugins/`. An existing
Docker config or foreign/dangling tool path is refused, then `docker buildx version`
must pass in the isolated environment. No installation or credential migration occurs.
It does not start k3s. These compatible releases intentionally contain the same Mail
binary with different image configuration labels/digests. `prepare` uses only supported
fresh migration/operator/enrollment/kernel CLIs and distinct TLS PostgreSQL containers.
It requires ready Traefik routes/endpoints and a default storage class. `import` verifies
archive hashes and exact CRI alias/config identities. `start` launches only reviewed
owned services and the actual enrolled Kubernetes agent; it never seeds installation,
runtime, customer or project rows with SQL. Proof queries use read-only transactions.
The node archive path must be absent, including no dangling symlink, before copying.
Its regular-file type, device/inode, size and SHA-256 are checked before import and again
before removal. The host archive UID is recorded before copying; only that exact UID or
root is accepted on the copied node file. If needed, only the verified file is changed
to UID0, with unchanged device/inode, link count, size and hash rechecked. Any other UID
or replaced path is preserved, not adopted or normalized.

The callback is an owned container on the existing dev Docker network. Its read-only
mounts are the exact kernel config/bindings/connections/invocation key/Core leaf/key/CA,
final catalog, platform public CA and one reviewed callback executable. It receives no
kernel issuer key, kubeconfig, metadata administrator file, environment-file mount or
bootstrap output. The installer controller alone reads issuer/admin inputs. The idle
callback catalog is finalized in place before serving so its file bind mount sees the
same exact bytes. No firewall rule or host-gateway route is added.

The target permits only callback DockerIP/32:callback-port, metadata DockerIP/32:5432,
and the fixture-owned management gateway DockerIP/32:8080, plus the native runtime's
existing cluster-DNS rules. The gateway replaces ephemeral Traefik PodIP/ServiceIP pins
in the Mail runtime path. It changes no Core/protocol/selector/schema contract; the
customer engine still denies all egress.

After observing the retained node's private Docker IP and waiting for Traefik rollout,
preparation runs `tools/tests/native-mail-gateway.mjs` in the reviewed Mail base image.
Exactly three read-only mounts provide the reviewed Bun ELF, gateway script and private
`mail-gateway.json`. It runs at the current UID, with capabilities dropped, a read-only
root filesystem, bounded tmpfs and no host publishing. It receives no kubeconfig,
issuer key, metadata administrator file or other credentials.

Its fixed upstream is `http://<observed-node-Docker-IP>:80`. It accepts only customer
Hosts `c<positive-digits>.mail-admin-<this-run>.localhost`, GET session/discovery paths
and POST `/jmap`. Bodies/responses and timeouts are bounded; redirects are never followed,
arbitrary upstreams/paths are denied, and authorization/body values are never logged.
Startup gates on health and a credential-free upstream HTTP check. An unallocated
customer Host may return404; a302 proves reachability but is blocked from forwarding.
The recipe's `mailboxes.admin.origin` uses the gateway's private8080 origin.

No host-network exception or stale endpoint allowlist is assumed. A failed health or
upstream check stops preparation with custody retained. Do not patch shared Traefik,
broaden policies or force the current fixture through. This is a local acceptance tool,
not a product service; no public reception or relay is added.

Browser acceptance performs real signed Console install, supported customer/project
and feature/placement operations, then explicit first mailbox creation through Console.
Install/domain/dummy-relay steps assert zero incoming allocation. A second project reuses
the customer's real CE engine. A different real customer cannot read the first mailbox.
Generation1→2→3 checks predecessor Pods gone, exact images, retained metadata database/role
OIDs and password verifier, engine/admin and mailbox ciphertexts, account/workload/PVC/PV
identity, original mailbox password unsealing and a retained JMAP draft. It also exercises
observed suspend/resume and incoming-send fail-closed. The JMAP probe is an owned,
loopback-only port-forward to the already owned engine Service; it stores a draft in the
existing mailbox, creates no account and submits no message. SMTP remains unused.
Before browser execution or standalone observation, current source/binary hashes and the
local-context guard must pass; cleanup stays exempt. Private probes wait boundedly for
both the mailbox row and the actual incoming customer state to be ready. API errors or
failed engine states propagate without hidden retries. Deny-egress proof accepts an API-
omitted empty egress list only with the Egress policy type and exact engine pod selector.

Custody is synced before mutation, then confirmed with returned container IDs, process
start-time/command identities, namespace/PVC/PV UIDs and image aliases.
Post-exec process capture waits for the real executable/hash and exact arguments with
the same PID start time and UID; the early parent/fork command line is never accepted.
Reviewed app/tool copies are hash-bound; existing system Docker/kubectl launchers are
resolved only from the fixed system PATH and their public ELF identity is pinned before
spawn. Process termination still checks exact recorded start time/command identity and,
for new records, executable/hash/UID.

Cleanup checks
those exact identities and uses Kubernetes UID/resourceVersion deletion preconditions.
It stops the callback container too: killing a Docker exec client does not stop its
remote process. A resumed cleanup honors verified absence and skips an already removed
callback and removed namespaces; a restored node's API need not remain live for those.
Daemon errors and mismatched identity still stop cleanup. Runtime namespaces precede
customer storage deletion; bound PV custody includes the exact claim UID.
Only recorded containers/anonymous volumes/tags/aliases
and private run files are removed. It never prunes shared caches or volumes. Initially
stopped retained k3s is restored to stopped; sanitized report/custody and root input remain.
Import also records attributable containerd digest/index aliases and the pre-import
reference set. After Pods are gone, cleanup checks each extra alias's exact target and
absence provenance before removing it, then handles the canonical CRI reference. Shared
or retargeted references are refused; no content GC runs.

Failed stages retain custody and a lock while active. An uncertain result without a
confirmed identity, a partial browser run, a foreign UID or an unknown private file
requires root reconciliation; the helpers do not adopt, recreate or blindly retry it.
Live topology, loader compatibility, JMAP behavior and complete cleanup remain acceptance
gates until the separately authorized run succeeds.

## Verified local boundary

The owned acceptance ran signed releases 0.1.0 → 0.2.0 → 0.1.0 in runtime generations 1 → 2 → 3.
It preserved metadata ownership/password, sealed administrator and mailbox material,
customer engine/PVC/PV identity, original mailbox authentication and the stored draft,
including incoming suspend/resume. A second project reused the same customer engine;
a foreign customer received an authorization denial without resource or credential data.
Both 403 and 404 are valid denied outcomes. Installing Mail and configuring outgoing-only
resources created no engine, and incoming API sends remained blocked.

The fixture used an owned fixed-upstream management gateway to avoid binding runtime
egress to changing Traefik Pod addresses. The gateway is test infrastructure; production
still requires a reviewed stable private management endpoint. All owned resources and
private generated material were removed, and the retained dev node was restored to its
initial stopped state. Existing mobile client-setting rows wrap long hostnames tightly;
the 375-pixel view remains usable without horizontal overflow.
