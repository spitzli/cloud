# Webdock observation and managed execution bridge

Source update, 2026-10-10: fixed-size persistent app storage and the native Mail provisioning profile are installed on production Contabo. See [storage operation and installation boundary](../../docs/architecture/persistent-app-storage.md). The normal agent keeps its existing sandbox; `storage_client.py` talks to the separately installed root-only `storage_service.py`. Customer BYOK installation does not install or enable that privileged helper. The standalone SSH bridge includes the client dependency in its transmitted code.

Use `--manage` to process authenticated Webdock app operations in addition to node observations. Without that flag, the bridge remains read-only. Active mode accepts only the fixed typed executor protocol: controlled namespace/Pod/Service manifests, safety dry-runs, revision/lease/ownership checks, rollout observation, bounded logs and confirmed application deletion. It has no arbitrary tenant shell or YAML interface.

Production now runs `daemon.py` as the enabled `webdock-hosting-agent.service` on Contabo. It connects directly to `https://auth.webdock.dev`, observes the node and processes managed operations every 15 seconds. Code: `/opt/webdock-hosting-agent/`; root-only connection state: `/var/lib/webdock-hosting-agent/connection.json`. No workstation or SSH process is needed. The local bridge below is an optional development tool and has been stopped. Lifecycle operations were verified on the real node with the local control plane before rollout; production enrollment, heartbeats and rendered administrator pages were verified separately. See `docs/verification/2026-10-08-hosting-production.json`.

```sh
python3 agents/kubernetes/connect.py \
  --ssh root@213.136.65.22 \
  --identity ~/.ssh/codeberg_1p.pub \
  --credentials /private/spitzli-agent.json \
  --manage --interval 15
```

HTTPS is mandatory except for explicit local `--allow-loopback` verification. Credentials must have mode 0600 and stay outside Git. Operations are sent through the authenticated Webdock service; never call the executor directly as a substitute for Webdock deletion authorization and its journal.

Supported now: cached immutable HTTP images, positive CPU/RAM/ephemeral quotas, restricted Pods, temporary /tmp, controlled ClusterIP services. Persistent PVCs require the enforcing storage helper; external image pulls require the operator-installed registry allowlist. New managed project/validation namespaces are distinct from the server baseline namespaces.

Managed apply operations observe readiness for at most 180 seconds. Both executor
transports allow 260 seconds for preparation and observation; the Auth operation
lease is 300 seconds. Lease expiry still interrupts work and timeout requires
reconciliation. Deploy the matching control-plane and agent changes together.
See [the production timeout evidence](../../docs/verification/2026-10-10-lunares-actions-production.md).

Run tests with `python3 -m unittest discover -s agents/kubernetes -v`.

## Historical observer-only background (superseded by managed daemon above)


This is a local SSH observation bridge, not a Kubernetes workload executor. It reads nodes and sends allowlisted metadata to Webdock. It never installs remote files, reads Kubernetes Secrets, changes namespaces or runs arbitrary commands from the control plane. Deployable capacity stays zero until a future executor verifies scheduling reserves, network, storage and ingress capabilities.

Run the tests:

```sh
python3 -m unittest discover -s agents/kubernetes -v
```

After protected Studio enrollment, store the agent response in a private mode-0600 JSON file with `endpoint` (control origin), `clusterID`, `credential`, `generation` and `sequence: 0`. Do not put the file in Git or paste its credentials in chat. Supply the public identity path for the existing SSH agent; never copy a private key into this application.

```sh
python3 agents/kubernetes/connect.py \
  --ssh root@213.136.65.22 \
  --identity ~/.ssh/codeberg_1p.pub \
  --credentials /private/webdock-observer.json \
  --once
```

HTTPS is required except for explicit local testing with `--allow-loopback` and a loopback origin. Omit `--once` to observe every 30 seconds. SSH host-key checking is mandatory. Redirects are rejected before credentials could be forwarded. Errors stop the bridge and do not change cluster state. A revoked agent cannot reconnect without a new enrollment.

The original connection was local-only; the owner subsequently requested and authorized production rollout while retaining Studio/Auth on Vercel. Server location evidence and local-path storage limitations are documented in `docs/architecture/hosting-control-plane.md`.

## Native Mail agent profile

Managed Mail uses the same authenticated daemon, with `mail-claim`,
`mail-checkpoint` and `mail-complete`. Only platform-owned k3s clusters participate.
Each explicitly activated customer receives namespace `wd-mail-CUSTOMER_ID`, a
retained fixed-size storage allocation and one restricted Stalwart CE container.
Suspension scales the existing Deployment to zero without deleting its PVC/data.

Managed installation must include `mail_executor.py`, `mail_client.py` and their
existing executor/storage dependencies. The observer-only SSH bridge and BYOK
installer deliberately do not advertise or execute managed Mail.

Before enabling the capability, build `infra/mail/Dockerfile`, import the exact
immutable image into the node's k3s image store, and create root-owned mode-0600
`/etc/webdock-mail.json` with `{"node":"YOUR_NODE","image":"REGISTRY/IMAGE@sha256:DIGEST"}`.
The observer verifies that digest is cached and that the cluster has exactly that
single node with an available storage helper. It never pulls arbitrary images.
Adding nodes requires explicit image distribution/scheduling support first.
Configure the same digest, the platform cluster ID, hostname suffix and independent
Mail encryption key in Auth (`MAIL_STALWART_IMAGE`, `MAIL_MANAGED_CLUSTER_ID`,
`MAIL_HOSTNAME_SUFFIX`, `MAIL_ENCRYPTION_KEY`). Apply the native Mail migration before
enabling `SPITZLI_NATIVE_MAIL_ENABLED`. Existing instance keys must remain decryptable.

Bootstrap runs in recovery mode while listener configuration is prepared. Permanent
credentials are checkpointed to Auth before the recovery environment and temporary
Kubernetes Secret are removed. Unknown mutations or lost leases require operator
reconciliation; the agent does not replay them. Internal ports are HTTP 8080, SMTP
2525, TLS submission 2465 and IMAPS 1993; no NodePort, public ingress or relay egress
is provisioned by this profile yet. A successful provisioning proof confirms the
private instance, **not** working public email delivery.

The real local restricted-container test requires the locally built image and an
explicit Unix Docker socket; it only creates isolated disposable fixtures:

```sh
SPITZLI_MAIL_DOCKER_TEST=1 DOCKER_HOST=unix:///var/run/docker.sock \
  python3 -m unittest discover -s agents/kubernetes -p test_mail_client.py -v
```

The private Mail profile was deployed on 2026-10-10 and verified through Studio
activation, suspension and resume for the platform-owned Spitzli customer. See
[production evidence](../../docs/verification/2026-10-10-native-mail-rollout.json).
Both running and stopped Deployments use the same server-side-apply manager;
using plain `kubectl replace` transfers replica ownership and breaks resume.
Previously verified instances in `needs_review` can be retried explicitly by a
current platform operator through Studio. Unknown first bootstrap outcomes stay
blocked for manual recovery; they are never automatically replayed.
