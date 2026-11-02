# Retired TypeScript platform

The active API is the Rust service in `crates/control-plane`; the active React Console and account UI are in `apps/console`. These applications are excluded from the root Bun workspace:

- `control-plane/`: the previous Bun API and workers, including its old plans/billing system.
- `console/` and `auth/`: the previous Next.js frontends.
- `mail/`: the previous standalone TypeScript Mail API and worker.

`tools/release/`, `deploy/`, `distribution-workflow.yml` and `docs/` retain the corresponding historical release path. They are references, not supported native build/deployment commands. The workflow is outside `.github/workflows` and cannot publish the retired runtime from this branch. Do not revive these writers alongside Rust.

`control-plane/drizzle/` remains a required build input for the Rust offline migrator: its checksums verify retained database history. Archiving source does not authorize dropping historical tables, resetting customer data, rotating keys or changing deployed processes.

Remaining native acceptance gaps are recorded in `docs/verification/2026-10-20-native-parity.md` at the repository root. The public website and its CMS integrations remain separate active TypeScript applications; they are not the platform API.
