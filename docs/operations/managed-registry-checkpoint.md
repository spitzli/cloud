# Managed Registry checkpoint — 2026-10-18

Admin setup is implemented and locally verified. Start from `bun run setup:admin -- host:port/database new-private-output.json`; follow `docs/developers/initial-setup.mdx`. Existing production operators were not changed.

The new external plugin source is `/home/newt/Projekte/Personal/spitzli/webdock-extension-managed-registry`, separate from the core. It manages an operator-provisioned dedicated Tabularium instance (branding, settings, kinds, manifest fields, provider readiness) through11 declared API/MCP actions. It does not yet provision new hosting resources or domains.

Local verification:15 unit tests,59 assertions; all11 actions against the deployed Tabularium0.25 image with a disposable PostgreSQL18; signed-runtime browser saves through the script-only iframe; nonroot read-only container/private TLS/public UI isolation; actual Webdock CLI manifest validation/pack; independent code/security review; Mintlify build validation and zero broken links. Evidence: `docs/verification/2026-10-18-managed-registry.json` and `2026-10-18-admin-setup.json`.

Local container `webdock-managed-registry:0.1.0-alpha.1` has manifest-list digest `sha256:d13685f9b345e949761fd3d2ee522a7960cd3d00afc6fe327c8d91bb8428da7c`. The manifest names the eventual GHCR reference; it is not uploaded. Descriptor SHA256: `516597fca9d6485386f820d9055b7cf09ace2ffbac9f1208c307f78fbbe00306`. Artifacts are in plugin `dist/`; English native screenshots are in `screenshots/` and explicitly show a local authoring harness/fictitious Harbor project. Screenshots are native captures at the preview's available quality.

Publication stopped because1Password did not respond to the signed git commit within45seconds. The process was timed out; no commit was created and all27 files remain staged. No remote repo, push, public image, GitHub release, registry submission or production plugin deployment was performed. Do not bypass the configured signing requirement. User explicitly requested shutdown on this condition.

Resume: unlock1Password, inspect staged source and successful evidence, sign the initial plugin commit, create public `spitzli/webdock-extension-managed-registry`, push source and image, verify the remote digest and anonymous pull, release version`0.1.0-alpha.1` with descriptor/assets, then submit through authenticated Webdock Registry. Existing registry MCP is authenticated as its admin, but token/session validity must be checked afresh. Existing production registry settings, especially external docs URL`https://docs.webdock.dev/`, must be preserved. Adopt it through its actual project`104618288083894272`, not the separate main project.

Core and Stall worktrees still contain earlier uncommitted work; no broad core deployment or migration has been performed. The core retention guard intentionally refuses unresolved legacy product inventory.


## Publication and architecture correction — 2026-10-18 daytime

The signed plugin source is now public at https://github.com/spitzli/webdock-extension-managed-registry (release commit 60851f0, tag v0.1.0-alpha.1). GitHub verified the initial SSH commit signature. The alpha release contains tabularium.json and managed-registry-universal.json with identical bytes, release-lock.json and both screenshots. The reviewed OCI digest above is public; anonymous Docker pull succeeded. Fresh build, 15 unit tests / 59 assertions, eleven real-provider actions and image smoke passed.

Registry submission required refreshing the existing GitHub OAuth login, then succeeded. The entry https://registry.webdock.dev/plugins/managed-registry is approved, not marked verified. Registry integrity is available with key ID 5d8025c66487bb44 and the expected descriptor hash; independent cryptographic verification was not yet executed. GitHub webhook automatic installation returned 403; no manual webhook was created (the prepared helper was interrupted before receiving its input).

The user challenged the architecture: Tabularium should be a managed app inside a Managed Apps plugin, rather than its own Managed Registry plugin. Further standalone rollout and docs publication stopped for this correction. No Managed Registry production runtime or host installation was created. Keep the public alpha as existing evidence; do not delete or yank it without a deliberate follow-up decision. The current source scan found hosting contracts but no independent managed-apps plugin repository.

Docs and screenshots are prepared separately in /tmp/webdock-registry-docs-release on docs/managed-registry-alpha based on upstream main. No docs commit, push or PR yet. Mint validation and broken links passed before review fixes; repeat validation after the final direction is applied. Independent review found missing dual-asset publishing instructions and stale Clerk transport/configuration; both were corrected in that temporary checkout, with smaller related consistency fixes. Main/core worktrees remain unchanged by this publication.
