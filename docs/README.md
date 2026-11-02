# Spitzli Cloud documentation

This `docs/` directory is the canonical Mintlify source. Public pages are English;
product links use `spitzli.cloud` and `console.spitzli.cloud`. Registered OAuth,
passkey and MCP protocol addresses retain their established compatibility values.

## Validate locally

Use Node.js 24. The qualified CLI for this update is `mint@4.2.1184`:

```bash
cd docs
npx --yes mint@4.2.1184 validate
npx --yes mint@4.2.1184 broken-links --check-anchors --check-redirects
```

The first command checks content, configuration, navigation and OpenAPI, then runs
the strict local site build. It does not publish. For a local visual review:

```bash
npx --yes mint@4.2.1184 dev --no-open
```

The repository contract check runs from the project root:

```bash
bun docs/scripts/export-mail-openapi.ts --check
```

Do not place credentials in examples or start real Mail sends as a documentation
check. Our own GitHub CI uses Blacksmith exclusively. The owner waived EU-only
placement for this scope on 2026-11-01; the organization uses US West and the
isolated managed-builder probe passed. Regular repository workflow publication
and source checks remain pending. The former runner is not a fallback. See
`tools/plugin-ci/BLACKSMITH.md` from the repository root. This exception does not
cover Mintlify builds or other services.

## Public source and publication

The canonical repository is `spitzli/cloud`; the old `spitzli/webdock` GitHub URL
redirects to it. The intended production connection is branch `main`, directory
`docs`, with domain `docs.spitzli.cloud`. Confirm those settings on the existing
Mintlify project before publishing; do not create a second site or replace DNS
as a substitute.

On 2026-11-01, the live custom domain served the Mintlify Starter Kit, not these
pages. GitHub confirms that earlier docs commits published automatically to
`docs.webdock.dev`; its latest README-only commit skipped the Mintlify check. Both the available browser and CLI were signed out of
Mintlify. Consequently the active publication source/branch has not been verified
and local changes alone are not evidence of a live deployment. Review the concrete
docs-only diff, preserve unrelated workspace work and the registered Git signing
identity, then publish through the confirmed connection. Verify the actual domain
and representative pages afterwards. Do not publish internal records.

## Structure

- `index.mdx`, `quickstart.mdx`, `guides/`: customer tasks and product availability.
- `developers/`: API/authentication, plugins, provider contracts and contribution.
- `openapi/mail.json`: generated retained Mail contract reference, 29 operations.
- `docs.json`: navigation, branding and reference generation.
- `.mintignore`: explicit public/internal publication boundary.

Architecture, operations, plans, verification, handovers, raw workspace API notes
and plugin ideas remain internal repository records. Navigation omission alone is
not a publication boundary. Preserve these records; do not rewrite old evidence
to imply a current rollout.

## Refresh the Mail reference

```bash
bun docs/scripts/export-mail-openapi.ts
bun docs/scripts/export-mail-openapi.ts --check
```

The exporter imports `mailOpenAPI()` from `legacy/control-plane`, assigns readable
operation titles/groups and the API navigation origin
`https://api.spitzli.cloud/api/mail/v1`. Paths, operation IDs, authentication,
parameters and schemas come from that retained source unchanged. This does not
establish native Rust endpoint parity or enabled production workers. The public
Mail guide explains availability and the reference playground stays disabled.

## Maintain accuracy

Read `AGENTS.md`. Verify account behavior against native Core/Console source and
current rollout evidence. Neon sign-in does not replace Core authorization or MFA;
a customer Neon provider connection is separate from platform identity. New shared
and self-hosted database orders are closed; existing-resource maintenance remains.
Build success, deployment readiness, OCI publication, registry signatures and plugin
activation are separate states. Provider catalog records require their own runtime
bindings. Never describe a TODO or a local qualification as generally live.
