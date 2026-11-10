# Hosting provider boundary

Source checkpoint: 2026-10-15. No cluster resources, provider projects or live credentials changed.

The existing Vercel project reader/list/delete client now lives in `adapters/hosting/vercel/src/projects.ts`. Its validation, team/project ownership checks, response allowlists, bounded reads, timeout handling, delete verification and provider error codes are unchanged. `@spitzli/hosting-contracts/vercel` remains a compatibility export. Data types and deployment-boundary descriptions live in `@spitzli/module-hosting/providers`.

The existing BYOK OAuth/control HTTP transport moved from `apps/control-plane/src/modules/hosting/byok-vercel.ts` to the adapter's `transport.ts`. API composition preserves its existing `HostingError` status/message contract. Credentials and team scope remain explicit arguments; the adapter has no database, app import, environment credential discovery or import-time network call. It keeps the fixed Vercel origin, no-redirect policy, 10-second timeout, two-megabyte response bound and redacted errors. Integration ownership, OAuth state consumption, account binding, credential encryption, operation leases and deployment publication remain existing service responsibilities.

Kubernetes already has an independent runtime boundary: `agents/kubernetes` executes enrolled, scoped operations on the cluster side. `adapters/hosting/kubernetes/runtime.json` identifies its real managed and BYOK distribution paths. It does not introduce a second HTTP control plane, move kubeconfig into the API or pretend the limited BYOK chart can replace managed storage/provisioning. Node-local credentials, owner/UID fences, admission checks, leases and retained-volume handling remain in the existing agent. A source move across the Python/TypeScript boundary would also need SSH-inline, systemd and archive migration and is deliberately not simulated with a fragile source symlink.

This establishes current provider boundaries, not generic cloud feature parity. Vercel and Kubernetes retain their different capabilities and execution models; no AWS/GCP transport or unsupported common cloud interface is invented.

Verification uses the existing offline Vercel project/deletion fixtures plus new transport tests for origin/team pinning, redirection refusal, response bounds and secret-safe failures. Agent runtime behavior is unchanged; distribution smoke is recorded in `selfhost-distribution.md`.

Local evidence: 20 project/deletion/transport fixture tests and 14 Git credential/publication fixture tests passed; `node node_modules/typescript/bin/tsc -p adapters/hosting/vercel/tsconfig.json` passed. These checks made no provider calls.

Scope clarification: new cloud services, including AWS/GCP additions, are explicitly excluded by the user’s selection. They are not an unfinished requirement of the existing-provider completion work.
