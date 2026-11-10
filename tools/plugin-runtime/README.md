# Private native plugin control

`render_control.py` renders the separately supervised callback listener and
installation controller using the existing immutable Core backend image. It
reuses the existing Core/Run clusters. It never creates a plugin metadata database
or gives a runtime provider credentials.

Render with explicit reviewed references; no secret values belong in arguments:

```sh
python3 tools/plugin-runtime/render_control.py \
  --image "$REVIEWED_BACKEND_IMAGE" --namespace spitzli-core --prefix webdock \
  --environment-secret spitzli-plugin-runtime \
  --source-secret webdock-native-plugin-host \
  --issuer-secret webdock-native-plugin-issuer \
  --callback-source 10.89.0.1 --runtime-host 10.0.0.2
```

This emits zero replicas by default. `--replicas 1` is an explicit activation;
rendering is not authorization to apply it. The addresses shown were observed on
this installation, not defaults for another cluster. The callback stays on a
private ClusterIP at TCP 3132; a separately reviewed, privately bound TLS
pass-through and host firewall provide Run connectivity. Never expose a NodePort
or public ingress. Callback sources must reflect the observed peer after relay
NAT; original Run sources must be constrained at the private host listener.

The three Secrets must be distinct. The dedicated environment contains only the
necessary restricted database pools, retained encryption/identity authority and
browser-client configuration. It must contain no database-owner URL, provider
administrator token or kernel CA key. Common source files include the retained
invocation key, Core leaf/key/public CA, private catalog and strict plugin config.
Only the controller's separate issuer source contains the CA signing key and
worker config. Empty `storageTargets` are required for the stateless Neon recipe.

Trusted init steps copy the projected source into bounded memory volumes as
UID65532, with directories mode0700 and files mode0600; they do not weaken the private reader.
Final private mounts are read-only. Pods have no service-account token, host
network, hostPath, engine socket, extra Linux capabilities or writable root.
Public HTTPS/database egress is limited by port and excludes private, loopback,
metadata and reserved IPv4 ranges. Only the observed private Run host is additionally
allowed on TCP443. This is a trusted Core process policy; the Neon runtime itself
receives only its exact callback egress and no provider or database endpoint.

Before activation, verify accepted SQL and the compatible rollback image,
retained key custody, exact signed release/image, target enrollment/capability,
real network-policy enforcement and mTLS rejection of invalid peers. Follow
`docs/operations/neon-plugin-rollout.md` for normal operator installation and
customer acceptance. TCP readiness alone does not prove successful installation.

```sh
python3 -m unittest discover -s tools/plugin-runtime -p 'test_*.py' -v
```

The focused tests run in Spitzli's Blacksmith CI. They cover fail-closed image and
address validation, issuer-secret separation, default-disabled singleton policy,
private service/ingress, memory staging and restricted pod settings. They do not
replace actual cluster or provider acceptance.

## Run namespace foundation

The Run cluster's application admission policy also applies to native plugin
runtimes. Keep that policy enabled. Each installation namespace needs a finite
ResourceQuota, LimitRange, default-deny policy and Restricted Pod Security before
its ready marker is set. Runtime containers require readiness, startup and
liveness probes. A catalog entry or an accepted install request proves none of
these prerequisites.

`harden_namespace.py` is an explicit infrastructure operation. Use the namespace
UID and runtime-spec fingerprint recorded by the current agent installation;
review the exact runtime NetworkPolicy against the approved catalog target and
hash its canonical JSON `spec` (sorted keys, compact separators). It refuses
foreign namespaces, changed existing boundary objects and a different isolation
policy. It verifies the persisted foundation before setting the ready marker
with a UID/resourceVersion compare-and-swap.

```sh
python3 tools/plugin-runtime/harden_namespace.py \
  --kubeconfig "$RUN_KUBECONFIG" \
  --namespace "$INSTALLATION_NAMESPACE" --namespace-uid "$NAMESPACE_UID" \
  --spec-sha256 "$RUNTIME_SPEC_SHA256" \
  --runtime-policy-sha256 "$REVIEWED_POLICY_SHA256" \
  --cpu-millicores "$REVIEWED_CPU_MILLICORES" \
  --memory-bytes "$REVIEWED_MEMORY_BYTES"
```

The default renders only. Add `--apply` for the reviewed infrastructure change.
The helper creates no tenant credentials, database, PVC, external service or
admission exception. The quota allows one bounded runtime Pod; `Recreate`
deployment strategy avoids parallel generations. Existing native-runtime egress
remains the approved callback and cluster DNS. The additional namespace policy
denies all other Pods by default.

This foundation is currently an operator prerequisite, separate from the generic
agent's six-object runtime checkpoint. Prepare and verify it for each new plugin
namespace, including generation-specific lifecycle namespaces. Do not describe
arbitrary new-cluster installations as unattended until this infrastructure step
is automated and qualified. Never work around a blocked install by setting only
the namespace ready label.
