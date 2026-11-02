# Webdock plugin contracts (implementation in progress)

This package currently provides the runtime-neutral `@webdock/plugin-sdk/contracts`
entry point: strict manifest validation, capability/method metadata, lifecycle
DTOs and a generated Tabularium extension schema. It builds compiled ESM and TypeScript
declarations; `bun pm pack` output is tested in an empty directory without a registry.

The server entry point provides invocation verification and scoped host-service
clients; the companion CLI scaffolds action runtimes. Method catalog entries do
not establish that every host service is implemented. Native installation, jobs
and complete third-party UI acceptance remain separate platform work.

Signed invocations require `customerID` alongside `projectID`. Core derives the
customer from the admitted project; plugins use this signed identity for shared
customer infrastructure and never accept a customer ID from action input.

```ts
import { parsePluginManifest } from '@webdock/plugin-sdk/contracts';
const manifest = parsePluginManifest(untrustedManifest);
```

Parsing validates shape and semantics, not registry trust. Hosts must verify release
identity/signature, exact manifest/descriptor bytes and image origin before using
this parser's result. Configuration/action schemas use a bounded local JSON Schema
subset with at most three nested child schemas; `$ref`, code and arbitrary schema
keywords are rejected. Images require a SHA-256 digest. Migrations are declarative;
checksum verification and execution belong to the future verified migration service.

From the repository root:

```sh
bun run --filter @webdock/plugin-sdk build
bun run test:modules
bun tools/plugins/generate-schema.ts --check
```
