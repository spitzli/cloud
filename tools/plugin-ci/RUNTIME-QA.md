# Explicit trusted RUN2 management qualification

This is a separately authorized, independently reviewed management test. It
creates only fresh owned QA namespace/PV/PVC/Pod/Secret/Service/policy objects and
one UUID filesystem root in the intended runtime cluster. It is not normal CI,
not arbitrary/customer code execution, and not an agent/controller activation.
No production config, PKI, SSH keys, RBAC, firewall or probes are changed.

The frozen source is checkpoint `534b9abdf1deb8896b626fd10dd23d1fa00dc51d`, archive
SHA-256 `17662f5588f401f7fa82a21c6442157a29a59ec63be04dfa28950b6a6dccf9b2`.
The reviewed fixture source SHA-256 is
`5ef4497b3f0ed6c3af31224d214620a2a847078794946056e06274d20bbbe8cf`.
All4554 other source files match the accepted full-test source; full lib58/7,
management16/0 and full-agent137/25 proof is explicitly carried forward.

The compiled fixture is available:

- Operator-local: `/tmp/webdock-diagnostic-runtime-qa-z0kd83vc/legacy-attachment-test`.
- BUILD: `/home/spitzli-plugin-ci/agent-candidates/diagnostic-17662f55-artifact/legacy-attachment-test`.
- Bytes: `356180960`.
- SHA-256: `087b91e9ac9ef6b27f62d2c7fa41354d440c4c04f47d8503115009041a76d632`.

Do not execute until the required RUN SSH authentication succeeds. RUN uses the
operator's existing default registered RSA agent; the local Ed25519 server key
is authorized only for BUILD. No fallback identity or added key is allowed.
The first transfer attempt stopped on `agent refused operation` / `Permission
denied(publickey)` before any RUN QA write or binary upload.

## Fixed profile and driver

`agent_runtime_qa.py` accepts only the compiled test path and its expected SHA.
There is no caller-supplied Kubernetes context or endpoint. Before every host or
Kubernetes write it checks:

- Physical DMI `91961f5a-b06b-44d1-9684-3f07ae5ca5bc`.
- Fixed root-owned canonical `/etc/rancher/k3s/k3s.yaml`, private mode/single link.
- Exact API `https://10.0.0.2:6443` and context `default`.
- Independent root-owned server CA PEM SHA
  `12381eae493b6fb156175929e9858d1c74770077d42ea12b115d8bffe0cf596a`.
- Node/hostname `wd-eu-02`, Node UID
  `29366860-fa4c-45c9-bb5c-69c54b038d4e`, machine ID
  `791bf8e3d5facd8bcee9a4c9c7965695`, physical identity.

It stages a checksum-rechecked root-only immutable test copy, checks ELF/shared
libraries, and executes exactly the reviewed ignored test with BOTH
`SPITZLI_RETAINED_QUALIFICATION_PROFILE=trusted-runtime-wd-eu-02` and
`SPITZLI_TRUSTED_RUNTIME_RETAINED_TEST=1`. Other explicit variables bind the fixed
Kubeconfig/context/node, fresh canonical UUID child under
`/opt/webdock-agent-qualification/retained-attachment-fixture`, and cached image
`ghcr.io/spitzli/tabularium@sha256:4627facbbdf045716949195b115c7ab5d823e8c2d810d8e3dda93eeaafd9e78f`.
No Kubeconfig or runtime credential enters the builder or Pod.

After authenticated read-only pin verification, transfer this driver and the
exact fixture into a root-owned private qualification directory. Execute once:

```sh
python3 /opt/webdock-agent-qualification/agent_runtime_qa.py \
  --test-binary /opt/webdock-agent-qualification/incoming-087b91e9-test \
  --expected-test-sha256 087b91e9ac9ef6b27f62d2c7fa41354d440c4c04f47d8503115009041a76d632
```

The driver emits `OWNED_RUNTIME_QA_POD_ACTIVE` immediately when its new owned Pod
runs. Notify the parent immediately for its DURING protected-object snapshot.
Parent owns the before/during/after38-protected-object UID/spec-SHA comparison
and Core/user API observations. After completion, notify cleanup result.

Private proof/logs include exact fixture exit status, passed1-test requirement,
fresh UUID root removal and namespace/PV inventory equality. A failure never
marks the candidate promotable. Preserve evidence and inspect only owned QA
resources; do not patch an actual app Deployment or relax policies to obtain a
passing readiness result.

## Accepted executable remains byte-pinned

The accepted agent is still SHA
`785cb77477644b1cba3fd4622809d20d810a12de6d7e3c2485d00d1fe6123a39` at
`/home/spitzli-plugin-ci/agent-candidates/bound-69a30142-artifact/spitzli-agent`.
A recompile produced `9e553b6ef53e7cfb05e4b83c6db00673627834154ebdd0c7cca5cee70f65de47`:
only `.strtab` and `.note.gnu.build-id` differ; every other ELF section including
`.text/.rodata/.data/.dynsym`, compiler provenance and package inventory matches.
That nondeterminism is documented in `agent-runtime-elf-equivalence.json`.
The strict byte-hash gate was not weakened and the accepted785cb file was not
replaced. Its GLIBC maximum2.34, actual BUILD glibc2.39 shared-library/help proof
and complete software checks remain available. No production promotion occurs
until the explicit live QA, cleanup and parent's protected-object checks pass.

## Actual first RUN2 execution

Required default RSA authentication resumed successfully. Fresh DMI/API/CA/node
pins passed; transferred driver SHA-256
`b5de405974a835811f6d0fe404f24a3a7673870b60c91aaf5bef46bcd45ce64a`
and compiled fixture `239c46b0…` were verified on RUN2 before execution.

The single owned qualification failed initial readiness with Rust test exit101.
PV/PVC reached Bound with exact UID checks. No Pod was admitted or became active:
RUN2's existing application-safety policy requires a fully hardened namespace,
but this fixture created no ResourceQuota/LimitRange and no ready label. The
Restricted PSA and deny NetworkPolicy were present. Do not merely add a ready
label or change admission policies. The fixture author must supply a reviewed
complete namespace bootstrap before another attempt.

Cleanup passed: namespace/PV inventory exactly restored and the owned fresh
UUID filesystem root removed. Proof is `agent-runtime-qa-attempt-1.json`; owned
admission evidence is `agent-runtime-qa-admission-evidence.json`. Private logs
are under `.runtime-test/plugin-releases-20261101/runtime-qa-attempt-1/`.
No production agent activation/config/PKI/key/RBAC/firewall/probe change occurred.
Parent owns the separate38-protected-resource AFTER comparison. The candidate
remains nonpromotable; no new build or rerun is authorized by this result alone.

## Reviewed hardened fixture refresh

Checkpoint61c9/fixture3d745e supplies complete owned quota, limits and policy
bootstrap with actual hardening verification before the namespace ready CAS.
Archive2479 differs only in the fixture; all4554 other files match accepted
software proof. The affected integration binary2365 compiled manually in EU
without a release build, passed ABI<=2.39 and exact ignored-test listing checks.
`agent-build-2479fc24-proof.json` records those gates and unchanged accepted785cb.
Parent supplied a fresh unchanged38-resource BEFORE snapshot and explicit GO
for one attempt. No GitHub workflow was triggered.

## Actual hardened RUN2 execution

One approved attempt ran with exact fixture2365 and driver920d after fresh fixed
pins and the parent's unchanged38-resource BEFORE baseline. Namespace
hardening and binding succeeded; initial Pod became Ready and the synthetic
retained data/environment read passed. The adversarial raced-binding update was
rejected. The following intended successful update returned generic
`Err(Runtime)` at frozen fixture line660, before stop/resume. An updated Pod was
observed Running. No exception, guard weakening or rerun was applied.

The test exited101. Cleanup independently passed: namespace/PV inventories
restored and UUID root removed. Parent captured DURING with all38 protected
UID/spec hashes unchanged and all four monitored API checks HTTP200; parent
owns the AFTER comparison. Proof: `agent-runtime-qa-attempt-2.json`. Private
fixture log and owned-only Pod timeline are under
`.runtime-test/plugin-releases-20261101/runtime-qa-attempt-2/`. The candidate
remains nonpromotable pending diagnosis, reviewed correction and full live pass.

## Narrow diagnostic attempt preparation

Attempt2 protected38 UID/spec proofs matched BEFORE/DURING/AFTER exactlyfc7b.
Independently reviewed checkpoint534b adds only seven subscriber setup lines:
`off,spitzli_agent::legacy=debug`, ANSI disabled, stderr, exclusive init. Existing
legacy API diagnostics expose code/reason/source, with no headers, API message
or Secret data. All4554 other source files remain identical.

Manual EU fixture-only compile087b passed ABI/listing/library checks without a
release build or full137 rerun. Driverf92b changes only archive/fixture hashes.
Parent supplied fresh BEFORE38 and GO for one diagnostic attempt. A failure is
evidence to diagnose; no blind whole-test retry or policy/source change follows.

## Actual diagnostic RUN2 execution

The single approved diagnostic attempt passed the entire exact ignored test:
1 passed,0 failed,0 ignored in43.44 seconds. This includes initial readiness and
retained data/environment, adversarial binding-race rejection, successful
foreign unchanged attachment update, stop/resume and unchanged storage-object
assertions. Cleanup passed independently: namespace/PV inventory restored and
the fresh UUID root removed. No policy exception or production activation ran.

The narrow logger emitted no legacy API error events. Attempt2's generic
`Err(Runtime)` did not reproduce; its API code/reason/callsite cause is therefore
unknown. The successful diagnostic attempt must not be reported as identifying
that earlier error. No automatic whole-test retry was performed.

Proof: `agent-runtime-qa-attempt-3.json`; private synthetic-only logs and Pod
timeline: `.runtime-test/plugin-releases-20261101/runtime-qa-attempt-3/`. Parent
BEFORE/DURING38 UID/spec hashes matchfc7b; AFTER receipt is pending. Accepted
agent785cb remains byte-pinned, with full software proof carried forward.

## Final protected-resource receipt

Parent independently confirmed BEFORE/DURING/AFTER all38 protected UID/spec
hashes exactly equal `fc7b4c241856321ba1a3b4e96d02980f86491ee88106c7f871b25cdf940d9c18`.
Receipt: `.runtime-test/plugin-releases-20261101/runtime-qa-protected-pass.json`.
Together with the actual full live-flow/cleanup pass and carried software/ABI
proof, parent approved only the byte-pinned accepted agent785cb for its own
controlled staging/activation. This harness did not activate production.
Attempt2's earlier generic Runtime remains unreproduced and unexplained.
