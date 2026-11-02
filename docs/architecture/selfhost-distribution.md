# Self-host distribution candidate

Source checkpoint: 2026-10-15. No artifact published and no cluster mutated.

## Boundaries

`deploy/docker/Dockerfile` builds Console, Auth, API, worker and the coordinated offline migration target from the root lockfile. API keeps workspace module/adapter sources because exports and FSR imports resolve at runtime. Next standalone traces from the repository root; public/static assets are explicitly included. Secret files are excluded by Dockerfile-specific ignore rules. Canonical origins and provider configuration are runtime settings; no public-origin or provider-secret build argument is required.

Compose consumes separate private environment files and immutable image references. PostgreSQL is external; loopback Docker test fixtures remain under the API's existing ownership-checked fixture commands. There is no default database password or owner credential in the distribution.

The control-plane Helm chart is namespace scoped and disables service-account token automount. Required external roles, CA and environment secrets are not created by Helm. Migration is an explicit Job with a separate Secret, not a lifecycle hook; operators must finish it before starting or upgrading applications. All networking is denied until explicit operator rules are supplied.

The separate optional agent chart runs the existing **BYOK** daemon with namespace-scoped permissions matching `agents/kubernetes/permissions.py`. It does not turn the node-local managed executor into a privileged container. Read-only nodes/namespaces/storageclasses inventory is the only cluster-wide RBAC. Application Secret read/write, exec, namespace creation, PV management and cluster-admin are excluded. Private enrollment state is copied only on first start into a preexisting PVC, preserving sequence across restarts. Re-enrollment/state replacement remains a stopped, explicit operator action.

Managed hosting and Managed Mail continue through the existing node-local agent/storage trust boundary. The isolated Mail image reuses `infra/mail/Dockerfile`; no shared CE tenant or eager instance appears in platform distribution. The binding mail-deployment-decision remains authoritative.

## Release path

The manually dispatched candidate workflow uses `[self-hosted, linux, x64, eu, ephemeral]` runners and SHA-pinned actions. Build jobs have contents-read only; only checksum signing requests OIDC. Runners must provide Docker Buildx with OCI/SBOM support, Node 24.15+, Helm 3, Python 3, skopeo, and cosign. Runner placement and destruction are operator guarantees, not facts a YAML label can prove. The workflow does not run on untrusted pull requests.

`tools/release/build.sh` requires an explicit native target platform and creates OCI image archives, Helm packages, a source revision manifest and SHA-256 checksums. BuildKit emits provenance and SBOM attestations. `manifest.mjs` rejects incomplete releases and verifies file size/digests; its unit test covers tampering and path traversal. The signing job signs the checksum list; the explicit publish script verifies signature, checksums and manifest before any registry mutation. Operator-supplied registry auth and GitHub auth are not embedded. OCI charts are pushed with `helm push`; GitHub releases remain drafts. Publishing is not deployment.

The build context currently includes the source workspaces in the API image, including development dependencies needed by the offline installer. A later minimal dependency packaging pass must preserve dynamic imports and independently verify all domains before reducing it.

## Verification gates

Run `python3 tools/release/check-charts.py` for positive and negative chart/schema assertions and `bun test tools/release/manifest.test.mjs` for integrity assertions. Full image builds and isolated fresh/existing-database migration tests must pass before release. A schema-valid chart with default-deny networking is not an operational deployment. Live TLS ingress, provider credentials, SMTP routing, actual runner EU location, release signatures and remote registry publishing require separate operational evidence.

## Local build evidence

On 2026-10-15 the Console, Auth and BYOK agent Docker images built successfully. Console/Auth standalone static chunks returned HTTP 200 in network-isolated containers with read-only filesystems and all capabilities dropped. The agent entrypoint ran under the same restrictions. Console's authenticated root requires a configured API and was not claimed as an end-to-end smoke. Chart render/schema rejection assertions, Compose configuration/restriction assertions and release manifest integrity tests passed. After coordinated Clerk, Mail UI and database integration, API, worker, migrator, Console and Auth targets also built successfully. Restricted network-isolated containers passed API SDK tests (6), worker pool tests (1) and migrator configuration tests (3); both rebuilt frontends served static chunks with HTTP 200. These are local build/package checks, not an authenticated deployment or a published release.

## Generated configuration and architecture evidence

The Helm `modules` object schema is generated by `tools/release/generate-chart-schema.ts` from actual module manifests through the SDK's `installationJSONSchema`. A drift check compares the inventory with the API registry. Unknown modules/properties, nonboolean flags, unsupported states and mixed enabled/state forms fail Helm validation. CI uses `--check` and never silently rewrites the schema. The chart serializes the same object into all relevant environments and the rollout checksum; workers remain opt-in.

Release manifests use schema version 2 and record SDK/protocol versions, OCI root-index and executable-image digests, archive checksums and native architecture execution evidence tied to the exact image indexes. `verify-architecture.mjs` refuses emulator/native-platform mismatch and executes API registry tests, worker-pool tests, migrator-config tests, frontend HTTP asset checks, Python agent entrypoint and Stalwart version in the built images. Those are runtime/package checks, not authenticated provider smoke. Missing checks, mismatched image evidence or unsupported architecture claims block manifest creation/verification.

CI remains on the existing x64 EU ephemeral runner labels only. There is no claimed or configured ARM runner; ARM64 is not locally verified. An explicit arm64 candidate requires a native arm64 Docker runner and every image execution gate to pass. Runner geography/ephemerality still requires operator evidence.

## Optional database distribution

`deploy/docker/database-gateway.Dockerfile` builds the existing Node gateway bundle on the explicitly supplied pinned Tabularis runtime. `deploy/docker/tabularis-runtime.Dockerfile` preserves that same immutable runtime for registered deployments. Source identity remains `vendor/tabularis-web-ui.json`; reconstruction uses its recorded base commit and vendored patches, not an invented download. The checked local runtime image was available and is not a source-packaging blocker.

The separate `webdock-databases` chart and `compose.databases.yaml` profile are disabled unless selected. They deploy only the gateway, not a customer database server or per-user registered runtime. The chart requires an existing private environment Secret, image digest, exact runtime commit and explicit networking. Managed process creation remains an opt-in runtime setting. Registered private HTTPS runtimes can use an existing CA ConfigMap; PostgreSQL per-session CA material still comes through the authenticated control channel. Managed children retain the existing shared-container/UID boundary; private directories are not an OS isolation boundary against a compromised process.

Release candidates include this profile only with `WEBDOCK_RELEASE_DATABASES=true` and an immutable `TABULARIS_IMAGE` matching the vendor record. The manifest then requires database-gateway and tabularis-runtime OCI artifacts, the optional chart, source commit/tree identity and additional native runtime evidence. The existing opt-in packaged-runtime test runs before evidence is recorded. The default seven-image candidate does not claim it contains the database runtime add-on.

Local evidence: the new gateway image built successfully and the real packaged-runtime test passed against a disposable PostgreSQL18/TLS fixture. It verified the runtime commit handshake, SQL/schema edits, verified TLS and wrong-CA refusal, cross-connection denial, revocation and temporary-state cleanup under nonroot/read-only/capability-drop restrictions. No customer database or cluster was touched.

The native release gate additionally evaluates two different canonical-origin/module selections inside the same built API image. This is runtime configuration/package evidence; frontend static checks remain static checks and are not labelled authenticated end-to-end verification.
