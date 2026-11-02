# Actions publisher and private registry on the existing application node

Status: **runtime installed and acceptance checked on October 10, 2026**. The
registry, publisher and versioned agent are active; actual application release
verification is a separate control-plane operation. Sanitized runtime evidence is
kept in `.superpowers/sdd/lunares-autodeploy/runtime-setup.json` (ignored local file).
The initial October 10 read-only inspection of
`root@213.136.65.22` (`vmd208517`) found 6 CPUs, 11 GiB RAM (about 10 GiB available),
188 GiB free disk, k3s/containerd, and no registry, skopeo, nginx, Node or Vercel CLI.
This adds no purchased infrastructure. Customer builds remain on GitHub Actions;
this node downloads, validates and publishes artifacts only. Never run `bun install --frozen-lockfile`,
Dockerfile builds, `vercel build`, customer scripts or guest.py on this node.

## Deployment inputs and prerequisites

Record the actual customer ID, project ID, control URL, storage-location evidence,
GitHub App Actions-read approval and enrolled artifact-only worker credential.
Lunares project is `101713101337919488`; verify the customer ID from trusted control
records. Replace `CUSTOMER_ID` below before installation. The sole repository is:

```
127.0.0.1:5443/customers/CUSTOMER_ID/projects/101713101337919488
```

This address works for **this single node**: kubelet/containerd pulls from host
loopback, not pod loopback. Adding another node requires an explicit registry
migration; never silently schedule these images onto another node.

Use signed Ubuntu packages for `docker-registry`, `nginx`, `skopeo` and
`apache2-utils` (htpasswd). Read-only apt-cache candidates on this node were
`2.8.2+ds1-1ubuntu0.24.04.3`, `1.24.0-2ubuntu7.18` and
`1.13.3+ds1-2ubuntu0.24.04.3`, respectively. Recheck security updates at execution.
Prevent package-service autostart during installation using the host's approved
package procedure; do not replace an existing policy-rc.d. Default nginx must not
start on port 80, which belongs to the existing ingress. Use only the custom units
below. Install a checksum-verified official Node 24 distribution, not Ubuntu's
Node 18 candidate, and operator-installed Vercel CLI **63.1.2** at
`/usr/local/bin/vercel`. Installing these trusted tools is separate from customer
build execution. Verify `skopeo --version`, `node --version`, `vercel --version`.

Prepare dedicated unprivileged users `webdock-registry`, `webdock-registry-proxy`,
and `webdock-publisher`. A `webdock-registry-socket` group grants only the first two
users access to the backend socket. Create private directories:

```
/etc/webdock-registry/                         root, group webdock-registry-socket, 0750
/var/lib/webdock-registry/                     webdock-registry, 0700
/var/lib/webdock-registry-proxy/               webdock-registry-proxy, 0700
/etc/webdock-artifact-publisher/               root, group webdock-publisher, 0750
/var/lib/webdock-artifact-publisher/artifacts/  webdock-publisher, 0700
```

The installed runtime uses a dedicated 20 GiB ext4 loop filesystem at
`/var/lib/webdock-registry-data.ext4`, mounted with `loop,nodev,nosuid,noexec` onto
`/var/lib/webdock-registry`. Its mount unit is required by the registry service
through `RequiresMountsFor=/var/lib/webdock-registry`; this gives a hard registry
storage ceiling without changing application volumes. Alert before exhaustion and
keep 5 GiB free for import staging. The worker's artifact-store ceiling does **not**
limit registry storage. Do not enable registry deletion/GC until retained release
references and rollback protection are implemented; GC must use a stopped/read-only
registry. Do not delete existing `webdock.local` images.

## TLS and separate repository identities

Generate a private CA and a server certificate with IP SAN `127.0.0.1`, using
OpenSSL on the operator machine or root-only node directory. CA key stays outside
service access. The server private key is root-owned, group
`webdock-registry-proxy`, 0640; install certificate as `server.crt`, key as
`server.key`, CA as `ca.crt` under `/etc/webdock-registry`. No insecure TLS flags.
Example server certificate extensions:

```ini
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=IP:127.0.0.1
```

Create independent high-entropy `lunares-pull` and `lunares-publish` passwords using
an operator password generator. Generate bcrypt htpasswd entries with `htpasswd
-iB` reading passwords over stdin, never command arguments. Write
`/etc/webdock-registry/users.htpasswd` root:proxy-group 0640. Store publisher auth
in a root-owned 0640 JSON authfile readable by `webdock-publisher`; store pull auth
only in the two Kubernetes pull secrets. Never print either JSON or use CLI
password flags. No GitHub PAT is needed.

Both identities need registry reads (`GET`, `HEAD`) for protocol negotiation and
blob checks. Only publisher may write (`POST`, `PATCH`, `PUT`); neither may delete,
list repositories, access another repository, or mount blobs from another repo.
Basic auth alone in Distribution does not impose these method/repository rules;
the following proxy is the authorization boundary.

## Registry backend: Unix socket only

Install `/etc/webdock-registry/config.yml`, root:webdock-registry-socket 0640:

```yaml
version: 0.1
storage:
  filesystem:
    rootdirectory: /var/lib/webdock-registry
  delete:
    enabled: false
  redirect:
    disable: true
http:
  net: unix
  addr: /run/webdock-registry/registry.sock
  host: https://127.0.0.1:5443
  relativeurls: true
```

Use `/etc/systemd/system/webdock-private-registry.service`:

```ini
[Unit]
Description=Webdock private OCI registry backend
After=local-fs.target
RequiresMountsFor=/var/lib/webdock-registry
[Service]
User=webdock-registry
Group=webdock-registry-socket
ExecStart=/usr/bin/docker-registry serve /etc/webdock-registry/config.yml
Restart=on-failure
UMask=0007
RuntimeDirectory=webdock-registry
RuntimeDirectoryMode=0750
NoNewPrivileges=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/lib/webdock-registry /run/webdock-registry
RestrictAddressFamilies=AF_UNIX
MemoryMax=512M
CPUQuota=100%
TasksMax=64
[Install]
WantedBy=multi-user.target
```

Check the package binary path before enabling. Distribution supports Unix sockets;
there is no unauthenticated backend TCP listener to bypass the proxy.

## Loopback TLS proxy with exact repository authorization

Install `/etc/webdock-registry/nginx.conf`; replace `CUSTOMER_ID` in the regex.
The proxy service must be able to read certificate, key and password hashes.

```nginx
worker_processes 1;
pid /run/webdock-registry-proxy/nginx.pid;
error_log stderr warn;
events { worker_connections 128; }
http {
  access_log off;
  client_body_temp_path /var/lib/webdock-registry-proxy/body;
  proxy_temp_path /var/lib/webdock-registry-proxy/proxy;
  fastcgi_temp_path /var/lib/webdock-registry-proxy/fastcgi;
  uwsgi_temp_path /var/lib/webdock-registry-proxy/uwsgi;
  scgi_temp_path /var/lib/webdock-registry-proxy/scgi;
  map "$remote_user:$request_method:$uri" $registry_allowed {
    default 0;
    ~^:(GET|HEAD):/v2/$ 1;
    # Challenge unauthenticated clients that begin at the manifest/blob endpoint.
    ~^:(GET|HEAD):/v2/customers/CUSTOMER_ID/projects/101713101337919488/(manifests|blobs)/[A-Za-z0-9_:.+-]+$ 1;
    ~^lunares-(pull|publish):(GET|HEAD):/v2/$ 1;
    ~^lunares-(pull|publish):(GET|HEAD):/v2/customers/CUSTOMER_ID/projects/101713101337919488/(manifests|blobs)/[A-Za-z0-9_:.+-]+$ 1;
    ~^lunares-publish:(GET|HEAD|POST|PATCH|PUT):/v2/customers/CUSTOMER_ID/projects/101713101337919488/blobs/uploads/([A-Za-z0-9_-]+)?$ 1;
    "~^lunares-publish:PUT:/v2/customers/CUSTOMER_ID/projects/101713101337919488/manifests/(release-[0-9]+|sha256:[a-f0-9]{64})$" 1;
  }
  server {
    listen 127.0.0.1:5443 ssl;
    server_name 127.0.0.1;
    ssl_certificate /etc/webdock-registry/server.crt;
    ssl_certificate_key /etc/webdock-registry/server.key;
    ssl_protocols TLSv1.2 TLSv1.3;
    client_max_body_size 512m;
    client_body_timeout 180s;
    auth_basic "Webdock private registry";
    auth_basic_user_file /etc/webdock-registry/users.htpasswd;
    if ($registry_allowed = 0) { return 403; }
    if ($arg_from != "") { return 403; }
    if ($arg_mount != "") { return 403; }
    # Reject noncanonical path encodings; preserve legitimate upload-state query strings.
    if ($request_uri ~ "^[^?]*(%|//|\\.\\.)") { return 403; }
    location / {
      proxy_pass http://unix:/run/webdock-registry/registry.sock;
      proxy_set_header Host 127.0.0.1:5443;
      proxy_set_header X-Forwarded-Proto https;
      proxy_set_header Authorization "";
      proxy_request_buffering off;
      proxy_buffering off;
      proxy_read_timeout 180s;
      proxy_send_timeout 180s;
    }
  }
}
```

Install `/etc/systemd/system/webdock-registry-proxy.service`:

```ini
[Unit]
Description=Webdock loopback TLS registry authorization
After=webdock-private-registry.service
Requires=webdock-private-registry.service
[Service]
User=webdock-registry-proxy
Group=webdock-registry-proxy
SupplementaryGroups=webdock-registry-socket
ExecStartPre=/usr/sbin/nginx -t -c /etc/webdock-registry/nginx.conf
ExecStart=/usr/sbin/nginx -c /etc/webdock-registry/nginx.conf -g "daemon off;"
Restart=on-failure
RuntimeDirectory=webdock-registry-proxy
RuntimeDirectoryMode=0700
UMask=0077
NoNewPrivileges=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/run/webdock-registry-proxy /var/lib/webdock-registry-proxy
RestrictAddressFamilies=AF_UNIX AF_INET
MemoryMax=128M
CPUQuota=50%
TasksMax=32
[Install]
WantedBy=multi-user.target
```

Verify syntax and socket access before enabling; test this exact auth policy with
real skopeo before calling it verified. A write-only publisher in product language
still needs read methods for blob existence checks; the pull identity has no write
methods. No host firewall port opening is required. Containerd begins with an unauthenticated
manifest HEAD, so the exact-repository anonymous GET/HEAD route must reach
`auth_basic` and receive a 401 challenge rather than being rejected early with 403.

## Containerd and skopeo CA trust, without restarting workloads

Live containerd already has
`config_path = "/var/lib/rancher/k3s/agent/etc/containerd/certs.d"`.
Create the registry-specific directory `127.0.0.1:5443` there and install public
CA `ca.crt` plus `hosts.toml` (root-owned, no credentials):

```toml
server = "https://127.0.0.1:5443"
[host."https://127.0.0.1:5443"]
  capabilities = ["pull", "resolve"]
  ca = "/var/lib/rancher/k3s/agent/etc/containerd/certs.d/127.0.0.1:5443/ca.crt"
```

Containerd reads changes under its existing hosts directory without daemon restart.
Install the same public CA at `/etc/containers/certs.d/127.0.0.1:5443/ca.crt` for
skopeo. Do not install pull credentials globally in containerd or registries.yaml;
Kubernetes supplies project-specific imagePullSecrets.

For persistence across future k3s starts/upgrades, also record the CA-only exact
host entry in `/etc/rancher/k3s/registries.yaml`, merging any existing contents:

```yaml
configs:
  "127.0.0.1:5443":
    tls:
      ca_file: /etc/webdock-registry/ca.crt
```

Do not restart k3s now. Its registries.yaml generation applies on a subsequent
restart; the already-configured containerd hosts directory provides current trust.
Verify it remains correct after the next planned upgrade/restart. Do not configure
wildcard mirrors, skip-verification, or global credentials.

## Upgrade only the trusted hosting-agent code

Installed agent release `2026-10-08-live-v1` has no registry.py/registry capability.
Copy the reviewed agent module set into a **new** root-owned version directory,
e.g. `/opt/webdock-hosting-agent-releases/ACTIONS_RELEASE/`, rather than overwriting
the live directory. Include daemon.py, observer.py, executor.py, connect.py, byok.py,
storage_client.py and registry.py from the same reviewed checkout. Compile-check
those modules before changing the service. Compare changes against installed code
and preserve the custom Lunares network helper and the separate storage service.

Preserve `/var/lib/webdock-hosting-agent/connection.json` exactly: never re-enroll,
reset its sequence, print it or replace it with development credentials. Preserve
all existing systemd hardening and `storage.conf` drop-ins. Use a new drop-in:

```ini
[Service]
WorkingDirectory=/opt/webdock-hosting-agent-releases/ACTIONS_RELEASE
ExecStart=
ExecStart=/usr/bin/python3 /opt/webdock-hosting-agent-releases/ACTIONS_RELEASE/daemon.py --credentials /var/lib/webdock-hosting-agent/connection.json --interval 15
Environment=WEBDOCK_REGISTRY_CONFIG=/etc/webdock-registry/agent-policy.json
```

Prepare root-owned 0600 `agent-policy.json`:

```json
{"version":1,"euStorageEvidence":"ACTUAL_OPERATOR_LOCATION_AND_STORAGE_RECORD","projects":{"101713101337919488":{"repository":"127.0.0.1:5443/customers/CUSTOMER_ID/projects/101713101337919488","pullSecret":"lunares-registry-pull","readOnly":true,"credentialScopeEvidence":"RECORDED_READ_METHOD_AND_REPOSITORY_DENIAL_TESTS"}}}
```

Create `kubernetes.io/dockerconfigjson` secret `lunares-registry-pull` in **both**
`wd-101713101337919488` and `wdv-101713101337919488`, with auth host
`127.0.0.1:5443` and only the pull identity. Use root-private manifests piped to
`k3s kubectl apply -f -`; never pass passwords in command arguments or print
secret data. Registry capability 1 is advertised only when policy and both secret
types exist. It does not independently prove credentials work; real pull tests do.

Restart only `webdock-hosting-agent` after confirming no operation is executing;
observe its next heartbeat. Existing `webdock.local/...` images remain `Never`.
Only the exact allowed registry repository at immutable digest receives
`IfNotPresent` and the project pull secret. Other project/repository requests to
this registry fail closed, including cached images. On failure, restore the prior
ExecStart/WorkingDirectory drop-in and restart the agent, preserving credentials.

## Artifact publisher

Deploy reviewed build-worker modules to a versioned path. Install the supplied
`apps/build-worker/webdock-artifact-publisher.service`, adjusting its ExecStart to
that path. Its limits are 4 GiB RAM, two CPU cores and one serialized work slot.
The registry/proxy budgets add 640 MiB RAM and 1.5 CPUs; verify total headroom before
enabling. Python importer staging uses the service's private /tmp.

Root-owned 0640 publisher configuration, readable only by its service group:

```json
{
  "controlURL":"https://AUTH_HOST/api/hosting/git/worker",
  "credentialFile":"/etc/webdock-artifact-publisher/credential",
  "country":"ACTUAL_TWO_LETTER_COUNTRY",
  "locationEvidence":"ACTUAL_RECORD_OR_EXPLICIT_UNVERIFIED",
  "storageEvidence":"ACTUAL_PRIVATE_STORAGE_RECORD",
  "isolation":"artifact-only",
  "isolationEvidence":"Only validated Actions artifacts; no customer execution",
  "artifactRoot":"/var/lib/webdock-artifact-publisher/artifacts",
  "maxStoreBytes":21474836480,
  "registryProjects":{
    "101713101337919488":{
      "repository":"127.0.0.1:5443/customers/CUSTOMER_ID/projects/101713101337919488",
      "authFile":"/etc/webdock-artifact-publisher/lunares-publish.json"
    }
  }
}
```

Use `ZZ` with explicit “unverified” location evidence if physical location has not
been independently verified; an IP address does not prove location. The hosting
registry policy's `euStorageEvidence` must reference real verified evidence.
Configure control-plane `WEBDOCK_GIT_REGISTRY_HOST=127.0.0.1:5443` through its private
configuration channel. Do not print the rest of that environment. Enroll the
artifact-only role, store the returned credential privately, then enable the
publisher only after all following acceptance checks pass.

## Acceptance order and rollback

1. Confirm only `127.0.0.1:5443` listens; no public registry port or backend TCP.
2. Unauthenticated requests must fail; pull identity GET/HEAD works but every
   POST/PATCH/PUT/DELETE fails. Both identities must fail on another repository,
   catalog, encoded path traversal and cross-repository `mount`/`from` requests.
3. Push a validated existing Actions OCI artifact with `skopeo copy
   --preserve-digests --authfile /PRIVATE/PUBLISH_AUTH --digestfile /PRIVATE/DIGEST
   oci-archive:/PRIVATE/image.tar docker://127.0.0.1:5443/customers/CUSTOMER_ID/projects/101713101337919488:release-RELEASE_ID`.
   Compare returned manifest digest to the locally verified manifest. Use the
   authorized release path for production artifacts; do not fabricate release IDs.
4. Prove kubelet pull with the project pull secret in validation namespace before
   changing the application. Test registry credentials via private files/stdin;
   never `crictl --creds username:password` or `curl -u username:password` argv.
5. Verify agent heartbeat reports registryVersion 1 and existing application remains
   ready with its original digest and `Never` policy until a release is approved.
6. Import one successful GitHub Actions artifact, verify provenance, publish through
   existing fenced release flow, observe actual readiness, then verify rollback.
   Ambiguous publication enters reconciliation; do not blindly republish.
7. Disable publisher service to stop new work. Registry outage should not stop the
   existing running container; preserve cached immutable previous images and registry
   data for rollback. Removing a pull secret or registry trust is not a rollback.

Installed runtime acceptance: verified TLS, method/repository denial matrix,
trusted skopeo read and actual kubelet pull via the project secret passed. The
validation fixture contained no executable application and its temporary pod was
removed. k3s PID remained 844; the existing Lunares pod UID and restart count
remained unchanged during setup. Publisher enrollment and an authenticated empty
claim were verified. The host-specific publisher unit also requires/starts after
the registry proxy; the agent override is bounded to 1 GiB and two CPUs.

The private registry policy records precise country/EU placement as **unverified**
under the legacy `euStorageEvidence` field; it claims no backup verification.
Geographic metadata does not relax credential, repository, digest or namespace
checks.

References: [Distribution configuration](https://distribution.github.io/distribution/about/configuration/),
[containerd hosts configuration and live reload](https://github.com/containerd/containerd/blob/main/docs/hosts.md),
[k3s registry configuration](https://docs.k3s.io/installation/private-registry).
