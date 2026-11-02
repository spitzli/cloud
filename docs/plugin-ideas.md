# Plugin ideas

Products that belong in plugins, not in Core. Core gives every plugin the same tools: workloads on
the cluster, the resource index, plans and limits, usage measurement, verified domains and
notifications. A plugin adds the product on top.

## Billing (planned)

Invoices from what Core already measures and limits. Core stays free of prices and money.

- Reads the customer's plan, its limits and the measured usage (hourly samples) from Core.
- Prices per plan and per unit above the plan (memory, CPU, storage, resource counts).
- Monthly invoices, credit notes and dunning; a payment provider stays inside the plugin.
- Suspends customers through Core's existing archive only after the operator confirms.
- EU tax rules (reverse charge, VAT ID check) are its concern, not Core's.

## Builds from Git

Push to deploy for the hosting plugin.

- The GitHub repository plugin already resolves commits and fetches source archives.
- A build unit on the cluster (BuildKit or buildpacks as a Kubernetes Job) builds an image and
  pushes it to the installation's registry, then the hosting app deploys the digest.
- Webhooks on push, build logs, and preview deployments per pull request.

## Object storage (S3)

Buckets with keys per project, on an EU cluster. Also the target for database and mail backups.

## Backups

Scheduled backups and restore for dedicated PostgreSQL (CloudNativePG continuous backup to object
storage, point-in-time restore) and for mail engine volumes. Needs object storage first.

## Redis / Valkey

Like the PostgreSQL plugin: shared and dedicated instances, TLS by server name through Traefik,
passwords rotated declaratively.

## App volumes

Persistent volumes for hosting apps (WordPress, Nextcloud, uptime monitors). Core already counts
their storage against the plan.

## Cron jobs and workers

Apps without HTTP: background workers and scheduled jobs. Needs `CronJob` as a workload kind.

## App templates

One-click bundles of app, database and volume (n8n, WordPress, Plausible, Umami, Gitea), with the
database connection injected into the app's environment.

## DNS

Zones for customer domains: records for mail (MX, SPF, DKIM, DMARC) and app domains set
automatically, DNSSEC. The provider stays inside the plugin.

## Uptime and status pages

HTTP checks from outside the cluster, a public status page per project, notifications through
Core.

## Error tracking and analytics

Self-hosted, privacy-friendly analytics and error tracking as installable apps with a project-level
setup page.
