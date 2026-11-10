# Private OCI registry with S3 storage

This is an opt-in operator profile for an internal trusted-worker registry. It creates
a Deployment, immutable configuration, ClusterIP Service and NetworkPolicy. It does
not deploy a customer-facing registry product, migrate an existing registry, create
provider resources, grant customer registry access or perform garbage collection.
The initial driver qualification uses the existing pinned Distribution 3.1.2 image.

Before rendering, prepare a separate private bucket, reviewed EU destination and
bucket-scoped provider identity. The S3 identity needs only the documented registry
bucket operations: ListBucket, GetBucketLocation, ListBucketMultipartUploads, and
object Get/Put/Delete/ListMultipartUploadParts/AbortMultipartUpload. Do not reuse a
builder upload, backup, customer app or account-root identity.

`existingSecret` must already contain:

| Key | Purpose |
| --- | --- |
| `s3AccessKeyId` | Restricted registry bucket principal |
| `s3SecretAccessKey` | Its retained secret |
| `httpSecret` | Stable random upload-state signing secret |
| `htpasswd` | Bcrypt hashes for trusted internal registry users |
| `tls.crt` | Verified certificate chain for the registry hostname |
| `tls.key` | Matching retained private key |

Keep credentials in the existing private Secret management path, never Helm values,
Git, reports or screenshots. Rotation requires an explicit provider/key transition
and pod rollout; changing a Secret environment value does not update a running
process. Preserve the HTTP signing secret across restarts and ordinary upgrades.

Supply a digest-pinned `image`, nonempty `locationEvidence`, explicit `s3` values,
exact IPv4 storage `objectStoreCIDRs`, and both trusted `clientNamespaceSelector`
and `clientPodSelector` labels. Pod credentials and server TLS stay inside the
existing Secret. The profile runs non-root without cluster credentials, host mounts,
capabilities or a writable image filesystem. Only a bounded temporary directory is
writable. No local registry data volume is used.

The Service exposes TLS on port 5000 inside the cluster. Exposing a hostname through
the existing trusted routing layer is a separate reviewed operation. Selectors must
match genuinely trusted worker namespaces and pods; do not place customer workloads
in those namespaces or treat a customer-controlled label as an authorization fence.
Kubelet/node image-pull reachability and private credentials require separate
qualification for the actual cluster; this chart does not automatically open a
node/public network range. The internal health listener is not published by the
Service and performs actual S3 storage checks.

The S3 destination requires HTTPS with certificate verification. The policy admits
only DNS and the explicitly supplied storage CIDRs/port; changes in provider IPs
need reviewed policy updates. Neither the chart nor a prefix proves provider tenant
isolation: use a dedicated private registry bucket and its tested policy.

Client deletion, automatic upload purging, S3 download redirects and all scheduled
GC are disabled. Registry authentication therefore remains on the object read path.
Old release references are retained. Unreferenced storage may grow until a future
Core-authorized, reference-aware GC is implemented and tested against active
releases and rollback targets. Do not add a bucket-wide expiration rule or run a
generic registry prune. Provider Object Lock is not enabled by this profile.

The configuration is immutable. Changing endpoint, bucket or root layout is not an
ordinary Helm upgrade or a data migration; prepare and verify a new destination and
an explicitly authorized registry cutover instead. Never delete the original bucket
as a side effect of changing configuration or removing this chart.

Offline checks (render only, no cluster contact):

```sh
python3 -B -m unittest discover -s tools/tests -p test_registry_storage.py
```

Owned local driver acceptance, with existing Webdock MinIO already running:

```sh
python3 -B tools/tests/registry-storage.py
```

The fixture creates only fresh named buckets, a restricted user/policy and a pinned
temporary Distribution container. Its test-only loopback HTTP profile omits TLS;
the operator chart always requires TLS. It pushes a verified OCI archive and a
different newer image, verifies every manifest/blob and both retained tags, denies
unauthenticated/wrong-key access and client deletion, restarts without local data,
revokes only its own S3 user, proves failure, re-enables it and proves recovery.
Cleanup removes the exact owned registry/container volumes and provider fixtures;
private diagnostics remain in the ignored plan workspace. It does not migrate a
registry, deploy a real Kubernetes app or establish Wasabi compatibility.

Primary references checked 2026-10-25:
[Distribution S3 driver](https://distribution.github.io/distribution/storage-drivers/s3/),
[registry configuration](https://distribution.github.io/distribution/about/configuration/).
