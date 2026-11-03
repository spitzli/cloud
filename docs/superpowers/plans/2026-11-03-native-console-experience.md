# Native Console experience implementation plan

> **For agentic workers:** Use superpowers:executing-plans task-by-task. Steps use checkbox syntax.

**Goal:** A coherent, modern Console experience across Core, generic plugin panels and first-party plugin views.

**Architecture:** Refine the existing shared theme and UI recipes. Keep the host UI ABI, resource/action contracts and independent plugin ownership; mirror only plugin view changes into the owning repositories.

**Tech Stack:** Existing React, TanStack Router/Query, Tailwind, gettext and Playwright. No new UI framework or remote font dependency.

**Spec:** `docs/superpowers/specs/2026-11-03-native-console-experience-design.md`

## Constraints and review focus

- Preserve authorization, cancellation, idempotent retries and secret lifecycle.
- Preserve explicit full-width login controls and usable mobile touch targets.
- Preserve dark/light contrast, focus visibility and reduced-motion behavior.
- Long German labels/customer data must wrap without page overflow.
- Generic plugins and retained legacy screens must receive the same baseline.
- Existing PR60 and Core image work continue; do not overwrite unrelated work.

## Task 1: Shared visual foundation

Files: `apps/console/src/styles.css`, `ui.ts`, `common.tsx`, `Shell.tsx`.
Consumes existing theme variables and UI recipe exports. Adds only an additive
`ui.resourceGrid` recipe for plugin resource sections.

- [ ] Set neutral light/dark surfaces, readable sans display type and focused violet accents.
- [ ] Refine compact cards, tables and page spacing; natural-width actions and mobile touch targets.
- [ ] Preserve explicit `ui.cx(..., "w-full")` overrides in auth and other intentional controls.
- [ ] Run existing component tests, Console no-emit typecheck and scoped ESLint.

## Task 2: Every plugin surface

Files: `apps/console/src/plugins/Panel.tsx`, `ResourcePage.tsx`, and the existing
`plugins/{hosting,mail,neon,object-storage,postgres}/ui/index.tsx` resource grids;
archives and generic plugins already use the common recipes. Corresponding
`../cloud-plugin-<name>/ui/index.tsx` files remain byte-identical when modified.

- [ ] Adopt the shared resource grid; retain all action handlers and confirmations.
- [ ] Audit generic forms, tables, reveals, state/error views and context navigation.
- [ ] Run plugin-renderer/panel/reveal, Neon, Hosting, Mail, object-storage and archive tests.
- [ ] Check each modified independent view against the actual installed host ABI.

## Task 3: Concrete customer journeys

Files depend on findings in existing `ProjectPage.tsx`, `WorkspaceProjects.tsx`,
`Connections.tsx`, Hosting build/deploy, self-service plans and resource pages.

- [ ] Review workspace/project selection, Git connection/import and build/deploy states.
- [ ] Review plans, quotas and provider connections for explicit next actions and understandable copy.
- [ ] Fix demonstrated UI gaps while preserving backend contracts; translate new owned copy.
- [ ] Validate normal customer and authorized operator/read-only behavior separately.

## Task 4: EU artifacts and genuine visual acceptance

Reuse bounded explicit fixture entries and existing browser helpers; add focused
cases where a shared surface currently has no visual coverage.

- [ ] Freeze exact source, build production Console and fixtures on the existing EU builder.
- [ ] Check desktop/mobile, light/dark, English/German, keyboard focus and overflow.
- [ ] Re-run actual interaction/security fixture checks; capture genuine labeled screenshots.
- [ ] Prepare a new Vercel prebuilt candidate and qualify it before promotion.

## Task 5: Source publication and rollout

- [ ] Commit only owned Core/UI/docs changes and synchronized independent plugin UI changes.
- [ ] Create/link PRs and use Blacksmith checks; watch/end the turn when waiting.
- [ ] Preserve immutable runtime release assets and update presentation only from reviewed source.
- [ ] Complete controlled Core/runtime and customer acceptance, then promote the matching Console.
- [ ] Update docs/evidence and perform the requested GitHub App rename afterward.
