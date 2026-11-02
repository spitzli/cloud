# Managed PostgreSQL qualification

Implementation includes native ordering, operator pools/catalog, the CNPG agent, SQL isolation, lifecycle and logical migration. See the [operations runbook](../../docs/operations/managed-postgresql.md) and [qualification evidence](../../docs/verification/managed-postgresql-lifecycle-pilot.json). The initial qualification used disposable local infrastructure. The first internal Webdock production instance is now verified in the [production evidence](../../docs/verification/2026-10-12-webdock-postgresql-production.json). Backups and backup-dependent recovery remain unconfigured; their implementation is now authorized by the [2026-10-25 storage handover](../../docs/handovers/2026-10-25-s3-managed-storage.md). Earlier no-backup offers remain unchanged.

## Refresh the live inventory

Python 3 and existing operator SSH access are sufficient. Use the existing pinned SSH configuration containing `wd-eu-01`, `wd-eu-02`, `wd-eu-03`:

```sh
python3 infra/postgres/inventory.py \
  --ssh-config /private/operator/ssh_config \
  --output /private/evidence/postgresql-inventory.json
python3 -m unittest discover -s infra/postgres -p 'test_*.py' -v
```

The output file must not exist. An unsuccessful collection leaves no successful snapshot. The script executes fixed read-only commands, validates the expected three-node cluster and storage identities, and never exports pod environments, Secret objects, kubeconfig, credentials or volume data. Review the metadata before sharing it: names, node identity, filesystem mount paths and source hashes are operational information.

`observedAt`/`completedAt` delimit sequential collection, not an atomic snapshot. `requests` use pod-level overrides, regular containers, init-container peaks, restartable init sidecars and pod overhead. Completed pods are excluded; unassigned pods remain a separate obligation. `declaredLimits` totals only declared values; `unboundedLimitDimensions` explicitly identifies incomplete limits. Desired pod specs are not a replacement for scheduler/resize status or real usage measurements.

`guardedHeadroom` is `floor(allocatable × 0.8) − requests`; it is not a reservation, placement guarantee or entitlement. Compare a fresh control-plane ledger separately with the already-approved cluster ceiling. Never multiply that approved ceiling by 0.8 a second time. CPU/RAM reserved for paused resources and retained storage still count. Every snapshot unconditionally carries `admissionApproved: false`.

## Version provenance

`versions.env` records immutable registry manifest digests. CNPG, PostgreSQL and PgBouncer passed Cosign verification with GitHub Actions OIDC and the upstream repository identities. The selected Barman image did not have a discoverable signature in this verification; do not treat its digest as equivalent to verified provenance. Candidate compatibility does not establish storage, admission, SQL isolation, backup or restore qualification.

Verify the pinned images again before use:

```sh
cosign verify "$CNPG_IMAGE" \
  --certificate-identity-regexp '^https://github.com/cloudnative-pg/cloudnative-pg/.github/workflows/release-publish.yml@refs/tags/v1.30.1$' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
cosign verify "$POSTGRES_IMAGE" \
  --certificate-identity-regexp '^https://github.com/cloudnative-pg/postgres-containers/' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
cosign verify "$PGBOUNCER_IMAGE" \
  --certificate-identity-regexp '^https://github.com/cloudnative-pg/pgbouncer-containers/' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

Load the reviewed `versions.env` into these shell variables first. Cosign is an operator tool, not a runtime application dependency. The initial verification used Cosign 3.1.3 from the upstream GitHub release; its binary matched the release asset SHA-256. No tool or image was installed on the production nodes.

## Local qualification tools

Use a disposable cluster and explicit kubeconfig. `Pilot.Dockerfile` supplies the isolated k3s test node; `boundary.py` installs reviewed admission rules. `check_boundary.py`, `check_bounded_volume.py`, `check_sql_isolation.py`, `check_lifecycle.py` and `check_migration.py` exercise actual infrastructure and require explicit synthetic identities. Read their CLI help and guards before use. Never run destructive storage probes against customer data.

The checked-in evidence records completed local runs. The initial production overlay and external pg endpoint are verified; broader external client coverage and the 24-hour pilot remain outstanding. Barman provenance and all backup work remain TODO, not prerequisites for implementing the owner-authorized no-backup product.
