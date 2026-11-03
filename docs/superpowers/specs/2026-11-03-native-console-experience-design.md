# Native Console experience

The owner approved Vercel-like clarity and consistency for the Core and every
plugin. Preserve Spitzli Cloud branding, working authorization and existing
services. This extends the 2026-11-02 Neon work; it is not a new backend or a
provider-feature expansion.

Use neutral light/dark surfaces, restrained borders and compact spacing. Use the already installed Inter font for product text and headings, retain
Space Grotesk for the brand wordmark, and reserve JetBrains Mono for code, IDs and
logs. All fonts remain self-hosted; no new dependency or remote font service. Keep violet for brand and focus, with clear monochrome
primary actions. Actions should use their natural width, with explicit full-width
controls preserved for sign-in and intentional layouts. Desktop controls remain
compact; mobile controls retain usable touch targets and readable input text.

Core and all plugin controls use the existing Console recipes and shared state
components. Custom resource views share one spacing recipe. Generic manifest
panels receive the same typography, controls, table and state treatment. Retained
legacy database views inherit the same theme. Preserve secure embedded content
viewers and their boundaries; this does not claim every external editor is a
native React implementation.

Do not rewrite backend actions, limits, provider credentials, cancellation,
idempotency, scope checks, read-only preview or confirmation behavior for a visual
change. Keep existing project/workspace context and navigation. Review Git/import,
build versus deployment status, connections, plans/limits, forms, resource detail,
errors and empty states as concrete user journeys. Do not invent unsupported
provider functionality or paid actions just to fill a screen.

The first implementation pass changes shared primitives and consistently adopts
them in first-party custom plugin views. Subsequent fixes must follow actual
customer-flow findings. A CSS pass alone is not proof that every journey is ready.
Use current component/security tests and genuine isolated browser fixtures, in
light/dark and mobile/desktop, with clearly labeled synthetic data. New production
artifacts are built on the existing EU host and qualified before promotion.

Existing in-progress Core migration/runtime gates and watched PR60 remain intact.
Do not cancel its checks for cosmetic work or claim the current old Console
candidate contains these new styles. Keep unrelated staged work. Plugin runtime
images/release signatures remain immutable; UI source is maintained in its owning
Spitzli repositories and the platform compatibility copies stay synchronized.
GitHub App rebranding follows the working rollout. Do not shut down the PC.
