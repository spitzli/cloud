# Shared native Mail gateway on the managed cluster

`render.py` prepares one shared API/worker plus browser-gateway pod in
`webdock-mail-gateway`. It does **not** provision a Stalwart instance. Only explicitly
incoming-enabled customers have an isolated instance, managed by the hosting agent.

The renderer creates no secrets, volumes or PVCs. Before applying it:

1. Preserve the existing `MAIL_ENCRYPTION_KEY`. Provision a `mail-control` Secret in
   this namespace with that key, `MAIL_GATEWAY_SECRET` and a distinct
   `MAIL_SEND_WORKER_SECRET`. Auth must use the same respective secrets. The browser
   container receives only the gateway secret and cannot mount the spool.
2. Provision a dedicated, retained, backed-up `mail-payloads` PVC. Precreate its
   `payloads` directory owned by UID/GID 65532 with mode 0700. Do not use the customer
   Stalwart volume, ephemeral storage or a public blobstore. Retain the volume/key
   together across rollback; never remove data because decrypting failed.
3. Build both rootless images from the repository root, import them into the existing
   node's containerd image store, and pass their actual immutable repository digests.
   The manifest uses `imagePullPolicy: Never`; mutable tags are rejected. Registry
   publishing is not implied by this renderer.
4. Configure Auth's `MAIL_API_ORIGIN=https://api.webdock.dev` and
   `MAIL_GATEWAY_ORIGIN=https://mail.webdock.dev`. Set up those DNS records and verify
   the existing Traefik `webdock` certificate resolver. Keep customer feature flags
   off until migration, account, domain and quota checks are complete.
5. Render and inspect the resources, validate against the cluster, then apply using
   a dedicated field manager. Do not alter customer Deployment replica ownership.

```sh
python3 apps/mail/deploy/render.py \
  --api-image "$MAIL_API_IMAGE_DIGEST" \
  --gateway-image "$MAIL_GATEWAY_IMAGE_DIGEST" > mail-gateway.json
kubectl apply --dry-run=client -f mail-gateway.json
kubectl apply --server-side --field-manager=webdock-mail-gateway -f mail-gateway.json
kubectl -n webdock-mail-gateway rollout status deployment/mail-gateway
```

The public ingress exposes only `/v1/emails`, `/session` and `/s/`. Neither
`/internal/webmail` nor `/internal/smtp` has a public route. Check this externally
through Traefik after rollout; a renderer assertion is not an ingress runtime proof.
The DATA hook is reachable from managed Mail pods on 3139 and authenticates its
per-instance token. The browser gateway delegates over same-pod loopback. Network
policy permits public HTTPS for Auth/relay API, cluster DNS and scoped Mail pod ports
8080/2465; it does not permit direct external SMTP from the shared pod. Policies on
each customer instance separately restrict SMTP egress to the selected relay IPs.

One replica with `Recreate` avoids two consumers of the private RWO spool and preserves
in-memory Webmail session semantics. Rolling restart may require reopening Webmail;
committed queue jobs survive and uncertain handoffs are never automatically resent.
Health probes establish process liveness, not provider delivery or Auth readiness.

After rollout prove Auth authorization, public-path isolation, retained spool/key
restart, current SMTP configuration v3, TLS pin persistence, DATA rejection without
a valid reservation and actual egress enforcement before enabling a customer's send
path. Public incoming routing and forwarding remain separate unreleased work.

Local manifest tests: `python3 -m unittest discover -s apps/mail/deploy -p test_render.py`.
