# Existing v1 plugin CI and old source-import retirement

On 2026-11-01 all four existing public TypeScript-v1 plugins completed their
real independent install/build/test checks on the qualified EU runner:

| Repository | Successful run |
| --- | --- |
| `webdock-extension-project-notes` | https://github.com/spitzli/webdock-extension-project-notes/actions/runs/41295825457 |
| `webdock-extension-managed-analytics` | https://github.com/spitzli/webdock-extension-managed-analytics/actions/runs/41295829450 |
| `webdock-extension-managed-apps` | https://github.com/spitzli/webdock-extension-managed-apps/actions/runs/41295833752 |
| `webdock-extension-lumaserv` | https://github.com/spitzli/webdock-extension-lumaserv/actions/runs/41295838190 |

Each check used `cloud-plugin-eu2-trusted-01`; candidate-image jobs were skipped
because these dispatches requested checks only. No new logical version, tag,
published image or registry asset was created. All four released manifest and
universal-descriptor downloads were rechecked byte-identical and still match
the owner's fresh pinned-key JWS proof in
`.runtime-test/plugin-releases-20261101/existing-v1-proof.json`. Their image
references remain unchanged. No source/dependency fixes were necessary: the
parent's reviewed CI-only changes passed as prepared.

## Retired private hosted source-import path

The separate private repository `spitzli/webdock-plugin-builds` is not a product
plugin and must not enter the trusted persistent runner group or shared cache.
Its `.github/workflows/plugin-build.yml` (workflow ID `388583633`) was read before
retirement: it manually acquired arbitrary import source through scoped OIDC
and built it on unqualified GitHub-hosted `ubuntu-24.04`. That location does not
satisfy the owner's verified-EU execution requirement.

Before changing admission, the latest five runs were all completed; the last
was 2026-10-18. There were no active runs to interrupt. After the four public
plugin checks passed, the old workflow was disabled (`disabled_manually`) and
repository Actions were disabled (`enabled: false`). Both changes were read
back. The repository stays private and its HEAD remains
`e697afeb0b50c16b66643813f5adcd25cf1bf6fb`; source/history were not modified and no
artifact deletion or credential rotation was performed.

There was no migration or fallback into group 3. That group's owner-managed
15-repository allowlist was left untouched. Existing references in
`tools/plugins/github-actions/README.md` and
`apps/build-worker/plugin-installer.example.json` are historical configuration,
not permission to reactivate the hosted backend or point it at trusted CI.

The old caller (`apps/build-worker/plugin_actions.py`) checks that its configured
workflow is active before `/actions-prepare` and before handing off source. A
disabled workflow therefore fails closed (`trusted_actions_workflow_required`,
or an earlier Actions-disabled API rejection), without dispatching code to the
trusted runner. Arbitrary/customer source needs its own separately qualified EU
sandbox/control plane. This retirement supplies no such backend: the old hosted
import route is unavailable, and customer-source admission to trusted CI remains
closed.

Detailed evidence is in
`tools/plugin-ci/legacy-eu-checks-and-retirement-2026-11-01.json`.
