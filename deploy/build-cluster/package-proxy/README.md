# Build package gateway

This profile belongs only to `webdock-build-eu-01` (`167.86.75.144`). It permits
build Pods to make TLS connections to the explicit names in `hosts.allow` through
`10.83.0.1:3128`. It does not intercept TLS, store response bodies, accept plain
HTTP forwarding, or open unrestricted Internet egress from build Pods.

`manage.py --install --on-new-build-host` checks the host identity, addresses,
single-node inventory, namespace default-deny policy and absence of active native
build Jobs before changing anything. Run it as root from a complete copy of this
directory on the approved build server. It installs the exact Ubuntu Squid
package version recorded in the script, masks the stock Squid unit before package
installation, then installs the bounded service, its independent UID firewall,
and the qualification namespace's port-3128 NetworkPolicy. Configuration drift
fails closed. An intentional profile change additionally requires
`--update-owned-profile`; it still checks ownership and requires idle build Jobs.

The gateway runs as the maintained `proxy` UID 13 with no capabilities,
`NoNewPrivileges`, a read-only system, 256 MiB RAM, 64 tasks and one CPU. Its
private listener accepts only build-Pod addresses. Two independent checks deny
private, metadata, loopback, CGNAT and known platform/public RUN addresses:
Squid destination ACLs and an output firewall scoped to UID 13. All upstream IPv6
is denied by that firewall. This prevents an approved hostname resolving to a
forbidden destination from bypassing the boundary. DNS is restricted to the host
resolver. The script never flushes unrelated firewall tables or changes the RUN
cluster.

The fixed runner tools configuration supplies the proxy to BuildKit, Railpack
prepare and Mise. Dockerfile proxy build arguments use BuildKit's standard proxy
handling. Railpack receives proxy values through temporary BuildKit secrets;
they are not added to the generated image's deployment environment. Customer
configuration cannot override proxy controls. Custom Dockerfile `syntax` and
`check` directives are rejected because the platform selects its own frontend.

Railpack 0.40.1's generated Node runtime step installs `libatomic1` using Debian
repositories configured with plain HTTP by its upstream base image. Actual
qualification failed with the gateway's expected HTTP 403. The operator runner
now inserts a fixed HTTPS source upgrade immediately before only the generated
APT command identified by that release's exact command prefix and custom name.
It changes `http://deb.debian.org/` to `https://deb.debian.org/` in standard APT
source files. `deb.debian.org` is approved for HTTPS; port 80 and plain HTTP
forwarding remain denied. APT signatures, certificate validation, customer
commands and other repository URLs remain unchanged. Dockerfiles using HTTP APT
sources must explicitly select HTTPS sources themselves.

## Qualification

Run `python3 diagnose.py --on-new-build-host` as root only on the approved build
host. The fixed diagnostic uses a temporary credential-free namespace with
Restricted Pod admission, gVisor, nonroot execution, no capabilities, quotas,
deadlines and DNS/proxy-only egress. It deletes each Job and the namespace before
returning. Its DNS-rebinding probes temporarily pause only the gateway firewall
refresh timer and alter only the gateway's owned hosts file; `finally` restores
the file, reloads Squid and restarts the timer. Do not run it concurrently with
customer builds.

The actual 2026-10-29 host qualification passed:

- HTTPS certificate/hostname validation against npm, GHCR and Debian.
- Denial of direct public TCP, unapproved hosts, wrong ports and plain HTTP.
- Denial of private API, metadata, CGNAT, loopback, IPv6 and all three RUN hosts.
- Denial when an approved hostname was rebound to private, metadata or RUN IPs.
- Direct connections under UID 13 denied with kernel firewall counters proving
  the independent boundary, followed by a successful restored HTTPS request.

Local guards: `python3 -m unittest discover -s
deploy/build-cluster/package-proxy/tests -v`. Re-run actual qualification after
changing the host, package, allowlist, network, resolver or firewall profile.

The 2026-10-30 full-source Dockerfile test also observed Docker Hub redirecting
blobs to `production.cloudfront.docker.com`. This exact hostname is listed in
[Docker's own allowlist](https://docs.docker.com/desktop/enterprise/allow-list/).
It is now included alongside the existing Cloudflare endpoint. An idle, owned
installation can update only the reviewed endpoint list with
`manage.py --update-hosts --update-owned-profile --on-new-build-host`; it verifies
the installed profile first, parses/reloads Squid, and restores the previous list
if reload fails. No package upgrade or firewall broadening is part of that action.

## Limits

This is a small explicit package allowlist; projects needing other registries
fail until an operator reviews and qualifies their endpoints. TLS traffic can
carry customer data to permitted services, and malicious code can transform or
transmit any build secret it receives. This gateway does not prevent that.
Connection and process bounds are global; per-tenant proxy bandwidth accounting
is not implemented. The host timer restores owned rules after drift but does not
replace admission, host maintenance or an adversarial isolation review. Public
untrusted multi-tenant readiness remains gated separately.
