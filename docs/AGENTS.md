# Webdock documentation

This directory contains the public Mintlify site and internal repository records.

## Public documentation

- Public pages are `index.mdx`, `quickstart.mdx`, `guides/` and `developers/`.
- `openapi/mail.json` is generated from the API's Mail contract; never hand-edit it.
- `docs.json` owns branding, navigation and API-reference generation.
- Use English source text, sentence-case titles and concise task-focused content.
- Each MDX page needs `title` and `description` frontmatter, an explicit navigation entry and language-tagged code fences.
- Internal page links are root-relative, extensionless routes. Use built-in Mintlify components.

## Terminology and evidence

- Console is the browser UI at `https://console.spitzli.cloud`; the workspace directory remains `apps/console`.
- Studio protocol URLs, OAuth issuer and MCP audiences remain compatibility contracts. Never globally replace them with the Console domain.
- Distinguish workspaces, projects, applications, websites and database services.
- CMS instance IDs and Auth website binding IDs are different. Preserve decimal Snowflake IDs as strings.
- Verify claims against current source and dated evidence. Mark pilot, opt-in and incomplete rollout boundaries; do not describe old plans as delivered features.
- Keep credentials, customer-specific infrastructure, private inventories and operational evidence out of public pages.

## Internal records

`architecture/`, `operations/`, `superpowers/`, `verification/` and `ux/` remain repository material. `.mintignore` excludes them from the site. Preserve their contents unless explicitly asked to edit them; do not mass-convert them into public pages. A hidden navigation page is not an access-control boundary.

## Verification

From the repository root: `bun docs/scripts/export-mail-openapi.ts --check`.
From `docs/`: `mint validate` and `mint broken-links`.
Preview using `mint dev --no-open`; inspect the rendered navigation and pages.
Consult the official Mintlify documentation for current configuration and components.
