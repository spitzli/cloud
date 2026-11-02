<!-- BEGIN:nextjs-agent-rules -->

# This is NOT the Next.js you know

This version has breaking changes — APIs, conventions, and file structure may all differ from your training data. Read the relevant guide in `node_modules/next/dist/docs/` (resolved from this file's directory; in monorepos the `next` package may not be visible from the repo root) before writing any code. Heed deprecation notices.

This block is written and re-added by `next dev` — verify at `node_modules/next/dist/server/lib/generate-agent-files.js`. Removing it from a diff only re-creates the uncommitted change; committing it with your work keeps the tree clean.

<!-- END:nextjs-agent-rules -->

## Mail deployment decision

The owner selected Stalwart **Community Edition, one isolated instance per customer with explicitly activated incoming email** (mailboxes or forwarding), refined on 2026-10-11. Outgoing-only uses Webdock API → Turbo relay API without Stalwart; incoming uses Webdock API → Stalwart → Turbo. Do not provision Stalwart for outgoing-only or every new customer/project. Deactivation retains data; deletion is a separate operation. Follow `docs/architecture/mail-deployment-decision.md`; this supersedes the earlier shared-Enterprise recommendation. Webdock Mail and Webmail remain native; Enterprise BYOK is a Webdock entitlement independent of the Stalwart edition.

Managed Mail belongs on the existing Webdock Kubernetes/Contabo cluster (owner confirmation 2026-10-10). Reuse hosting-agent security/operations; Docker is the isolated local test backend, not the production control plane.

Private Mail provisioning is live on Contabo for the explicitly enabled Spitzli
customer. See `docs/verification/2026-10-10-native-mail-rollout.json` for deployment
IDs and the successful retained-volume suspend/resume test. This does not mean
public delivery, managed/BYOK relay or Webmail is complete. Preserve the Mail
encryption key, retained storage and server-side-apply replica ownership.

## Own GitHub CI: Blacksmith

Use Blacksmith exclusively for Spitzli's own GitHub CI. On 2026-11-01 the owner
explicitly waived EU-only placement for this CI scope; the current organization
setting is US West. This exception does not cover customer builds, deployments,
databases, storage or other services. Record actual runner/build evidence before
claiming qualification. Do not fall back to GitHub-hosted or the former persistent
self-hosted runner. See `tools/plugin-ci/BLACKSMITH.md` for setup and current gates.

## Public documentation build exception

On 2026-11-02 the owner explicitly allowed Mintlify to build the public
documentation without a confirmed EU build region. This exception applies only
to public documentation. It does not extend to customer builds, deployments,
databases, storage, authentication or other services. Do not describe Mintlify
as EU-only without provider evidence.
