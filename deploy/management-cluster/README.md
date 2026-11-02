# Core/OBS management-cluster foundation

Owner authorized Core/OBS setup and Contabo Private Networking. These profiles
reuse the existing pinned standalone K3s convention, never join RUN or BUILD and
never expose a Kubernetes API on a public address. They are not installed yet.

Contabo private network62595 (`webdock-core-obs-eu`,10.0.4.0/22) was created and
only the two new instances assigned: Core100210975→10.0.4.1 and
OBS100210955→10.0.4.2. OBS Private Networking addon was purchased through the
explicit upgrade field. Existing RUN network62202 and BUILD were not modified.

The provider reports network assignment status `reinstall` for both new hosts.
Their OS identities remained intact; neither a provider restart nor a subsequent
graceful cold cycle on Core exposed a second NIC. **Do not reinstall automatically.**
The owner must approve the destructive OS step before executing the prepared
private review request. Existing SSH policy/keys are verified, and no K3s/application
has been installed on the new hosts. Keep strict host-key verification in place.

## Offline checks

```sh
python3 -B -m unittest discover -s tools/tests -p test_management_foundation.py
bash -n deploy/management-cluster/bootstrap.sh
python3 deploy/management-cluster/profile.py core --output /private/core/profile
python3 deploy/management-cluster/profile.py obs --output /private/obs/profile
```

The two local profile tests first failed before implementation, then passed.
They verify fixed roles, separate Pod/service CIDRs, private API settings and
explicit host/drift guard presence; they do not qualify a live cluster/firewall.

After authorized provider activation and real private-address verification, copy
`bootstrap.sh` and the matching `profile/` to a private directory on the exact
new host. Run `bash bootstrap.sh --fresh-management-host core` (or `obs`). It
requires the exact hostname and already-present provider address before any
package/configuration mutation, and refuses foreign K3s or identity/config drift.

K3s is pinned to1.35.9+k3s1 with its verified binary SHA256, generated secrets and
kubeconfig remain private, and local-storage/Traefik/ServiceLB are disabled.
The API binds only the provider address; host input is default-deny except SSH,
ICMP, established traffic and local CNI API/kubelet access. Public eth0 forwarding
to Pod interfaces is denied. These are trusted infrastructure clusters, not an
untrusted build profile or public API ingress. TLS/mTLS services, storage quotas,
namespace admission and every peer exception require separate qualification.

Do not deploy the inert OBS candidate with missing secrets/storage or weaken its
deny-all rules. Do not generate replacement production encryption/database/PKI
keys. Preserve the separate REST and gRPC design and prepare the concrete first
backend migration/rollout/traffic change before its approval.
