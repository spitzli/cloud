# Plugin state bootstrap checkpoint

The runtime state foundation now requires an explicitly initialized installation before the API or worker starts. This is not the live-module cutover: current built-in admission and worker selection still use the existing configuration until Core Tasks 3–4 are complete. Do not expose the pending state-command service as a working operator feature yet.

## Prepare an existing installation

1. Retain the current effective `SPITZLI_MODULES` selection, domain ownership, encryption keys and database backup. No plugin data or provider resources are deleted by bootstrap.
2. Through the existing guarded offline migration workflow, apply the numbered migrations and reinstall policies. Use the dedicated offline role, exact target and verified CA; runtime roles cannot initialize state. `tools/migrations/install.ts` performs both migration and policy installation.
3. In that same explicit offline environment run:

```sh
bun --no-env-file tools/plugins/bootstrap.ts EXACT_HOST:PORT/DATABASE legacy
```

Pass the existing `SPITZLI_MODULES` value through the offline environment if it was configured. Its absence in the explicit offline legacy import means the five remaining product modules are enabled. This is separate from normal API startup, where absent configuration disables products. Billing is no longer registered; remove billing from the reviewed selection before importing. Repeat bootstrap preserves existing state; changing the environment does not overwrite it.

For a fresh installation use `fresh` instead and explicitly supply `SPITZLI_MODULES`, including an empty module selection if desired. Fresh initialization without a selection fails.

4. Start API/worker processes only after initialization succeeds. They verify forced RLS, required policy/helper presence and bootstrap state. API `/readyz` becomes unavailable if required initialization disappears. All replicas must retain the previous module configuration while live admission is unfinished.

No production migration or rollout was performed while implementing this checkpoint. Registry trust, runtime TLS, controllers, service credentials and the public plugin UI domain remain future cutover inputs.

## Persistence and access

Migration `0043` stores runtime generations, approved configuration/secret references, participant leases, job executions, invocation authority and immutable document mutation receipts. These records are retained; there is no purge operation.

Invocation scopes contain only a host-signed invocation ID alongside the verified session or job scope. RLS resolves installation/customer/project from the retained invocation record and checks active customer/project, grant revision, approved capability, configuration revision, active runtime generation, expiry and revocation. Interactive invocations additionally require the matching current session/member and no preview. Jobs require their enabled durable grant plus the exact active execution claim and lease; they never fabricate a browser identity.

The trusted document broker may see a scoped row with write authority to perform compare-and-swap updates. This is not a public read grant: Core Task 6 must explicitly require `plugin.data.read` for its read endpoint. Receipt replay instead requires `plugin.data.write` and the same operation. The broker/API and invocation-token endpoints are not delivered by this database checkpoint.
