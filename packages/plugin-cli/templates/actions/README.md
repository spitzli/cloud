# __NAME__

A stateless Webdock plugin for automatic source installation. Its `demo.echo` read action returns `{ "message": "..." }` for an input message of at most 1,000 characters and requests no capabilities. It has no jobs, tables, migrations or UI.

## Build and install

Use Bun 1.4 or newer:

```sh
bun install --ignore-scripts
bun run build
```

Commit the generated `bun.lock`, the `vendor/` directory, `tabularium.json`, Dockerfile and source. The CLI bundles its installed SDK in `vendor/plugin-sdk`; no published `@webdock/plugin-sdk` package is required. The SDK's public dependencies are still installed by Bun. The lockfile records the resolved dependency versions and the Dockerfile installs from it.

In Console, open the project's extensions and choose **Install plugin**. Select the repository through GitHub (private repositories require read-only Contents access), provide an HTTPS Git URL, or upload a ZIP up to 32 MiB. The installer builds the Dockerfile and pins the resulting image. Review the built manifest and configuration, then approve. Deployment and project activation follow automatically while the installer page is open. Keep that page open; refresh it if status updates pause.

The zero image digest in the source manifest is a scaffold placeholder. The installer replaces it with the actual built image digest before approval. Do not publish that placeholder as a release.

## Runtime contract

The installer supplies `WEBDOCK_PLUGIN_ID`, `WEBDOCK_PLUGIN_VERSION`, `WEBDOCK_PLUGIN_GENERATION`, `WEBDOCK_PLUGIN_HOST_URL`, `WEBDOCK_PLUGIN_RUNTIME_TOKEN`, `WEBDOCK_PLUGIN_PUBLIC_KEY`, `WEBDOCK_PLUGIN_CONFIGURATION`, `WEBDOCK_PLUGIN_TLS_CERT` and `WEBDOCK_PLUGIN_TLS_KEY`. Never put runtime tokens or TLS keys in source control. The certificate and key files are mounted read-only under `/runtime-tls`.

The process runs as the unprivileged `bun` user. It serves HTTPS on port 3443. Every request requires the runtime bearer credential. `GET /healthz` returns the exact plugin ID, version, generation and protocol version 1. `POST /actions/demo.echo` additionally verifies the signed `x-plugin-invocation` token, plugin and generation identity, expiry and exact action name before processing `{ "input": { "message": "hello" } }`. An SDK context is created per invocation with the configured host origin and runtime credential. The demo makes no host service calls.

Do not add jobs, tables or migrations for this source installer pilot; those lifecycle features are not supported. A browser panel, if added later, belongs on a separate plain HTTP listener on port 3444 behind the host's HTTPS ingress, never on the private action listener.
