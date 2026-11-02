# Local Barman source provenance prerequisite

This is an isolated **LOCAL FIXTURE ONLY**, not a production signer, backup qualification, image promotion, controller installation or product change. It never contacts a cluster, registry write API, S3 service or account. MinIO backup/restore tests remain a subsequent local step.

Requires Linux/amd64, local Docker with OCI image loading, Python 3.11+, Git, GPG/GPGV and OpenSSL with Ed25519. No global tools are installed. Allow approximately 4 GiB RAM for the largest container and several GiB of private disk space. Containers have two CPUs, at most 4 GiB memory (swap disabled), 512 PIDs and phase deadlines within a 90-minute Docker task budget. Smoke containers have no network, a read-only root, no capabilities, one CPU and 256 MiB RAM. Preparation has outbound access only to public source/dependency repositories; no host credentials or Docker socket are mounted. Docker's existing local image cache can retain the two immutable base pulls.

```sh
python3 -m unittest discover -s infra/postgres/backup -v
python3 -m unittest discover -s infra/postgres -p 'test_*.py' -v
python3 infra/postgres/backup/provenance.py build \
  --output /tmp/webdock-barman-proof-UNIQUE
```

`prepare` runs source/dependency preparation without producing images; `build` performs fresh preparation and then builds both components. Each requires a new private directory outside the repository. Failed builds retain a signed `preparation-only` statement with the failing command, logs and available evidence, and exit nonzero. They cannot pass `--require-images`. If both dependency preparations finished, `resume-build --output SAME_DIRECTORY --public-key-sha256 ORIGINAL_PUBLIC_PIN` verifies that signed preparation before retrying the offline builds. It retains the preparation recipe, previous signed statements and retired output artifacts/images, reuses only this task’s fixture identity, and creates new UUID-scoped containers. After success, retain the printed public-key SHA-256 separately, then verify with that exact pin:

```sh
python3 infra/postgres/backup/provenance.py verify \
  --output /tmp/webdock-barman-proof-UNIQUE \
  --public-key-sha256 PUBLIC_PIN_FROM_BUILD --require-images
python3 infra/postgres/backup/negative_checks.py \
  --output /tmp/webdock-barman-proof-UNIQUE \
  --public-key-sha256 PUBLIC_PIN_FROM_BUILD
```

Run negative checks only on an idle fixture. They change and restore a byte in real artifacts, check rejection, and verify the restored bundle. They test signatures, source archive/compiler inputs, both OCI layers, signed OS metadata/package bytes and a Python dependency artifact. Unit tests additionally reject a correctly signed statement for the wrong source and a digest-valid image with a substituted manager.

## Inputs and trust boundary

- Source: `cloudnative-pg/plugin-barman-cloud`, commit `6216b7dd552f389952d3de77712d9522a79d9548` (v0.15.1). `git verify-commit --raw` must report GitHub web-flow signer `968479A1AFF927E37D1A566BB5690EEEBB952194`. The public key is obtained from GitHub and matched to this explicit fingerprint. The signed Git tree is exported to `source.tar`; the extracted compiler input tree is checked against it before/after building and during verification. Python’s safe tar extraction can normalize a directory symlink’s trailing slash; the verifier permits that equivalence only. No source patches are applied.
- Go builder: `docker.io/library/golang@sha256:23fdfd3a6abc97c81e32a724cdd1cf541c06c416eb04d717815f4ed7c75623d0` (linux/amd64, Go 1.26.6).
- Python builder and both runtime roots: `docker.io/library/debian@sha256:4876fdea29cac1a33a3195f1eb1952feb1fca7dd6dd00be3e2f1879cf20d34d2` (linux/amd64, trixie-slim).
- The pinned Debian archive keyring SHA-256 is `506b815cbb32d9b6066b4a2aa524071e071761e7e7f68c3ac74f3061ba852017`. Each preparation locks the then-current signed Debian main/updates/security metadata and all exact installed package versions and hashes, including inherited packages. Obsolete base packages are upgraded from these authenticated repositories. Every installed `.deb` is downloaded and independently checked through `gpgv(InRelease) -> SHA256(Packages) -> SHA256(.deb)`. Historical package disappearance fails closed. Retained signed metadata is verified offline; a later run deliberately creates a new package lock, rather than silently claiming byte reproducibility.
- These explicit OCI digests and the keyring are bootstrap trust anchors from the official images. This does **not** independently reproduce or authenticate the build of those base images or the host. Host executable hashes, base image metadata, Go compiler identity, commands and actual container resource/network settings are retained.
- Go downloads use the signed source's `go.mod`/`go.sum`, the official module proxy and Go checksum database. `go mod verify` runs online after download and again in the network-disabled compilation container. Automatic Go toolchain download is disabled. The binary's linked module listing is retained.
- Python downloads enforce upstream's complete `containers/sidecar-requirements.txt` with `--require-hashes`, including Barman 3.20.1. Pip, setuptools, wheel, compiler and headers come from the authenticated Debian packages; no mutable pip upgrade or isolated build dependency download is allowed. Both offline builders reconstruct the exact authenticated OS package set using at most three non-forced dpkg passes for pre-dependency ordering, then require an empty dpkg audit and an exact installed-version comparison. Python wheels build and install in a separate network-disabled container using verified source/wheel downloads. Generated wheels have a new SHA-256 lock, `pip check` must pass, and installed distributions/files are inventoried. This authenticates Python artifacts through the signed source lock, not through an upstream Python release signature.

## Outputs and limitations

The recipe replaces the upstream mutable Dockerfiles. It builds the same Go manager for both components; the sidecar additionally receives the locally built Python virtualenv. Both use Debian runtime roots and UID/GID 65532, rather than upstream UBI/distroless roots and sidecar UID 26. Debian supplies libpq; the PGDG bootstrap is omitted. These differences need later compatibility and security qualification.

Each full runtime filesystem is exported from its exact owned container and packaged as a single-layer OCI image. The tool rehashes every OCI descriptor and uncompressed layer and checks packaged manager/virtualenv files against the built artifacts. Imported Docker manifest/config IDs, effective config and layer digests must match the packaged OCI content; smoke commands run by Docker’s immutable reported image ID (manifest with containerd, config with the legacy store). Docker’s containerd ID behavior is checked against its [implementation](https://github.com/moby/moby/blob/master/daemon/containerd/image_inspect.go). No upstream Barman image is mirrored, retagged or signed.

`provenance.json` is an in-toto Statement v1 with a Webdock local-fixture predicate and **both** OCI manifest digests as subjects. `provenance.sig` is a detached Ed25519 signature. It binds source, empty patch list, archived recipe, base/compiler/host identities, dependency/OS evidence, artifact hashes, smoke logs and both CycloneDX 1.6 SBOMs. The SBOMs enumerate installed Debian packages, linked Go modules and installed venv distributions; they are not an arbitrary vendored-code scanner. `fixture-private.pem` stays mode 0600 inside the mode 0700 private output directory. It is a newly generated test identity, **not operator approved**. Store its public pin separately; accepting a pin supplied by an untrusted bundle would defeat the verifier.

`ownership.json` lists exact UUID-scoped container names and imported local image names/Docker IDs/config digests/manifest digests. `commands.jsonl` retains commands and timeouts. Containers are removed by exact name in `finally`, including failures/timeouts. No volumes, custom networks or ports are created, and there is no prune. OCI layouts/tars, source, caches, evidence and fixture key are retained privately. Remove imported outputs only by the exact IDs in `ownership.json` after review (`docker image rm IMAGE_ID`); do not remove shared base images or use a prune. The source/dependency caches can contain root-owned read-only files; any eventual directory removal must target only the explicitly recorded private task directory.

No production image trust, operator-approved signer, vulnerability clearance, reproducible base build, resource admission, MinIO/S3 operation, backup, restore or PITR result is implied.
