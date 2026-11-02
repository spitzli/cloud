# Workload clusters

What a cluster needs before Core places plugin workloads on it. The local dev k3s installs the same
pieces from `deploy/dev/k3s`.

- Traefik with the entry points plugins route through: `web` (80), `websecure` (443), and for TCP by
  server name `postgres` (5432), `imaps` (993), `submissions` (465).
- cert-manager, and `acme-issuer.yaml` as the ClusterIssuer plugins use. HTTP-01 needs every routed
  host to resolve to the cluster and port 80 to reach Traefik.
- CloudNativePG for dedicated PostgreSQL.
- metrics-server, so usage shows what units actually use.
- The Webdock agent (`deploy/helm/webdock-agent`), enrolled with Core.

Each plugin's runtime binding names its cluster (`workloadCluster`) and the domains its units may route
without a customer verifying them (`routeDomains`, e.g. `apps.example.eu`). Customers' own domains need
their TXT verification in the console. Operators mark clusters as offered on the Clusters page; a
project then chooses one while nothing runs in it, otherwise each plugin's cluster applies.
