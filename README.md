# Spitzli Cloud

Spitzli Cloud is a Rust control plane with a React Console, local account security, workspaces, projects and explicitly enabled product plugins. Customer websites keep their own repositories and content stores.

The active platform API is `crates/control-plane`. The Console and account UI share `apps/console` (`@webdock/console`, Vite). The old TypeScript Control Plane, Next.js Console/Auth and Mail service are preserved under `legacy/` and excluded from the active Bun workspace. Their plans and billing implementation are historical references; the native Core owns the active plans, limits and usage system. Billing is a future plugin.

## Development

Use the pinned Rust toolchain, Bun 1.4.3, Docker and Tilt:

```sh
bun install --frozen-lockfile
bun run dev                 # Tilt: local database, Rust services, Console and account UI
bun run quality             # Rust, contracts, frontend and local browser fixtures
```

Console runs on 3121, account UI on `localhost:3125`, and Rust Core on 3130. Tilt stores private local configuration under `.dev-local/`. Use disposable fixtures for tests; never customer databases or real mail. Dev mail goes only to Mailpit. See [native development](crates/control-plane/README.md) and [Console development](apps/console/README.md).

Tilt also runs MinIO for local S3 tests: API `http://127.0.0.1:55900`, web console `http://127.0.0.1:55901`, region `us-east-1`. Use path-style addressing with S3 clients. The generated access key (`MINIO_ROOT_USER`) and secret key (`MINIO_ROOT_PASSWORD`) are in `.dev-local/minio.env`; reruns preserve them. Containers on the Compose network use `http://minio:9000`. Override host ports with `WEBDOCK_DEV_S3_PORT` and `WEBDOCK_DEV_MINIO_CONSOLE_PORT`. Buckets and objects live in the retained `minio` Docker volume. The first start builds the [pinned official MinIO release](https://github.com/minio/minio/releases/tag/RELEASE.2025-10-15T17-29-55Z) locally because its historical upstream image tags are unavailable.

For separately configured processes:

```sh
bun run dev:api
bun run dev:console
bun run dev:auth
bun run build:api
bun run build:console
```

`apps/website` is the separate public website; use `bun run dev:web`. Its CMS and TypeScript integrations are outside the platform API retirement.

## Runtime and installation

| Source | Responsibility |
| --- | --- |
| `crates/control-plane` | Rust HTTP API, account/OAuth protocols and first-party UI delivery |
| `apps/console` | React Console and canonical account screens |
| `crates/agent`, `crates/agent-control` | Authorized workload execution and reporting |
| `crates/plugin-host`, `crates/plugin-runtime` | Plugin admission, limits, workload pinning and RPC |
| `plugins/` | Independently built product providers |
| `crates/database-gateway` | Scoped access to native Tabularis runtimes |
| `legacy/` | Retired TypeScript applications and their distribution tooling |

Build the native container from the repository root:

```sh
docker build -f deploy/docker/Dockerfile --target core -t webdock-core:local .
docker build -f deploy/docker/Dockerfile --target migrate -t webdock-migrate:local .
bun run test:distribution
```

Core serves the built Console and account UI on their configured hosts. It needs private restricted-role PostgreSQL connections and canonical origins from [the runtime example](crates/control-plane/.env.example). Offline migration and initial operator setup use independent credentials and exact database targets; see [native setup](crates/control-plane/README.md). Runtime startup does not upgrade schemas. For the opt-in signed product-plugin initial path, see [native plugin installation](docs/operations/native-plugin-installation.md).

Native Compose and Helm candidates are under `deploy/`. Managed workloads belong on the existing Kubernetes cluster. The old multi-service Bun release pipeline is archived under `legacy/`; historical releases are not native Rust release evidence. This branch has no published native release or production cutover. Domain migration, retained-data upgrade/rollback and outstanding product parity need their own acceptance before rollout. See [the parity inventory](docs/verification/2026-10-20-native-parity.md).

## Data and compatibility

Platform IDs are decimal Snowflake strings. Keep existing OAuth issuers, MCP audiences, registered clients, identity/session keys and Mail encryption keys during migration. Filesystem renaming does not change protocol identifiers.

Incoming Mail is explicitly activated per customer with one isolated Stalwart Community Edition instance. Outgoing-only customers do not receive Stalwart. Suspension retains data; deletion is separate. See [the mail decision](docs/architecture/mail-deployment-decision.md).

## License

Original Spitzli Cloud code is **AGPL-3.0-only**. See [LICENSE](LICENSE), [alternative licensing](COMMERCIAL-LICENSING.md) and [contribution requirements](CONTRIBUTING.md). Third-party components keep their own licenses.
