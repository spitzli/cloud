# Public website hosting on Cloudflare

Owner direction, 2026-11-05: host `spitzli.cloud` and `spitzli.site` on Cloudflare.
Treat this as website hosting as well as DNS. The immediate scope is the public web setup. It does not automatically move Console,
Auth, API, customer workloads or databases. Preserve the existing `*.spitzli.site`
fallback-address service and inventory its routing before any change.

Current authenticated inventory:

- The `spitzli.cloud` zone is active in the Spitzli Cloudflare account. Its apex
  is an unproxied CNAME to Vercel and has no Worker custom-domain binding.
- The owner clarified the intended second domain is `spitzli.site`, already
  purchased. The earlier `spitzli.link` registration/price investigation is cancelled;
  no domain purchase is authorized or needed. Inventory the existing site zone and
  customer wildcard routing.
- The `apps/website` source is Next.js 16 with Payload CMS, dynamic rendering,
  request locale handling and a PostgreSQL-backed landing-page global. It is not
  currently a static export. Preserve CMS editing, published content, legal pages,
  image/OG routes, translations and waitlist CTA behavior during migration.

Implementation gates:

1. Confirm the exact live website project/artifact and CMS route dependencies.
   Inventory the `spitzli.site` apex and customer wildcard routes separately;
   do not replace a short-link application with a guessed landing page.
2. Qualify the applicable Cloudflare hosting path in an isolated candidate. Reuse
   existing Cloudflare configuration where present. For a project without one,
   follow the current Cloudflare CLI/product guidance; don't introduce an unrelated
   framework or unvalidated Next/Payload adapter just to satisfy a DNS check.
3. Preserve the existing EU data/execution requirements. Cloudflare's global edge
   is not proof of EU-only processing. Keep CMS/database/auth credentials and
   protected data on qualified infrastructure; explicitly review any new runtime
   processing/region requirement. Public asset hosting and protected server code
   have different requirements. No paid regional service is activated implicitly.
4. Build/validate with existing repository conventions. Own GitHub CI uses only
   Blacksmith; customer build/processing region restrictions remain separate.
   Test the candidate's actual content and application behavior, not HTTP200 alone.
5. Verify direct Cloudflare delivery, TLS, canonical URLs, locale behavior, CMS
   publishing, waitlist/access-request handoff, legal/OG assets, and every existing
   short-link redirect. Keep an immediately usable prior-host rollback.
6. Only then attach the domain/cut over its web DNS. Turning on the Cloudflare
   proxy in front of Vercel is not a completed hosting migration. Preserve mail,
   unrelated subdomains and working authentication callbacks.

This is an inventory/implementation boundary, not a completed deployment. No DNS,
hosting binding, credential, source framework or live service was changed during
this initial inventory.

## Corrected existing-domain scope

The owner confirmed **spitzli.site is already purchased**. Discard the earlier
spitzli.link purchase interpretation and budget question. No registration, transfer
or new recurring domain purchase is needed. Preserve customer wildcard addresses,
run-cluster deployments, TLS and isolation while qualifying the Cloudflare web setup.

Nameserver handoff verified after the owner saved Namecheap settings: both
spitzli.cloud and spitzli.site resolve to gemma.ns.cloudflare.com and
huxley.ns.cloudflare.com through both Cloudflare1.1.1.1 and Google8.8.8.8; both
zones are active in the existing Spitzli account. Existing wildcard site address
routing remains unchanged. This verifies DNS delegation, not the hosting migration.
