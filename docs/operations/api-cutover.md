# Native API deployment path

The active platform API is `crates/control-plane`, serving the built `apps/console` UI. Use the native candidates under `deploy/compose/` and `deploy/helm/webdock/`; the corresponding setup contract is in `crates/control-plane/README.md`.

The former Bun cutover runbook and manifests are preserved under `legacy/docs/operations/api-cutover.md` and `legacy/infra/api/`. They are historical records and must not start writers alongside Rust. The former Redis/Bun worker deployment is likewise archived under `legacy/infra/core-jobs/`.

This source change does not alter deployed infrastructure. Before any production transfer, verify the intended EU target, backup/restore, retained schema, identifiers, secret continuity, ingress/SSO, execution enrollment and rollback after new writes. The product/retained-data parity inventory remains `docs/verification/2026-10-20-native-parity.md`; native runtime selection is not parity or cutover evidence.
