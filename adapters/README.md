# Provider adapters

Adapters are grouped by product area, then provider. Each TypeScript leaf owns its package, public exports and tests. Bun discovers them through `adapters/*/*`; the area directories are organizational, not additional packages.

```text
adapters/
├── auth/
│   ├── better-auth/
│   └── clerk/
├── databases/
│   └── postgres/
├── hosting/
│   ├── github/
│   ├── kubernetes/
│   └── vercel/
├── mail/
│   └── turbosmtp/
└── storage/
    └── vercel/
```

Use package imports such as `@webdock/adapter-hosting-vercel/projects`, not relative imports across adapters. Package names are stable identifiers; directory grouping does not change them. Vercel hosting and Vercel storage remain separate because they implement different product contracts.

Product behavior belongs in `modules/<product>`. Shared contracts belong in `packages/`. The Control Plane composes services and adapters and supplies bound server-side credentials. An adapter must not import application internals or bypass module admission, tenant authorization or durable operation fences.

`hosting/kubernetes` describes the enrolled-agent boundary through `runtime.json`; it is not a JavaScript package. The actual Python executor stays in `agents/kubernetes`, with separate credentials, privileges and deployment artifacts.

Run `bun run test:modules` from the repository root to include adapter tests recursively. New providers belong under the relevant area; add another area only for a distinct product responsibility.
