# Webdock management architecture

Status: design only. The current delivery restores independent website CMS instances. No customer panel, generic collection designer or platform page builder is implemented.

## Deployment and data ownership

Each website keeps its own UI, repository, Vercel project, Payload configuration, `/admin`, users, sessions and migrations. The existing Neon database hosts `webdock`, `spitzli` and `stall` PostgreSQL schemas. Each runtime connects with its own restricted login. Schema names are immutable infrastructure labels, not future customer IDs. The retained `public` schema is the retired CMS source/rollback archive.

This separates application authorization and data access. CPU, database availability, backup retention and the current public Blob store remain shared infrastructure. A database outage can affect all three instances. Individual database branches/projects or buckets can be assigned later without changing the customer model. Public Blob originals and derivatives have been copied into each instance's own prefix; old objects remain for rollback.

Current Vercel production, preview and development credentials point at the same per-site schema. They are not separate testing databases. Provision isolated branches before using previews for schema experiments or untrusted changes. Automatic schema push stays disabled in every environment.

## Future control plane

An independent Webdock admin application manages infrastructure and ownership. It does not query every CMS to render its customer list, nor does every CMS request depend on this application. Provisioning, recovery and updates are explicit operations; established websites continue running if the control plane is unavailable.

Keep the first model small:

| Entity | Purpose |
| --- | --- |
| Customer | Organisation/contact and lifecycle, with no automatic CMS |
| Project | Customer website/service, domains, repository and hosting references |
| CMS instance | Optional project capability; template version, desired/current state, URLs and resource references |
| Operation | Durable provisioning/update/recovery attempt, checkpoints, sanitized error and operator |
| Audit event | Actor, operation, target, timestamp and outcome; no secrets or content bodies |

Platform-owned IDs use **Snowflake**, selected by the owner on 2026-10-03. IDs are decimal strings in JavaScript and APIs to avoid precision loss. PostgreSQL coordinates allocation for the initial single-region control plane; no per-process Vercel worker counter is used. Preserve existing Payload-issued identifiers and references in restored applications.

Store references to secrets in the control plane, not plaintext credentials in customer-readable records. Infrastructure credentials belong to Webdock; customer CMS accounts never receive database owner, Vercel or shared storage credentials.

## Optional CMS provisioning

Creating a customer or project does not create a database schema or deploy Payload. CMS is disabled by default. An explicit operator action selects a versioned template and starts a durable operation:

1. Reserve an immutable instance label and operation record; reject duplicate active provisioning.
2. Create a scoped database role/schema (or dedicated database when required), storage scope and unique instance secret.
3. Deploy the selected template without routing the production domain yet.
4. Run versioned migrations, provision the protected operator and initial customer account, then validate authentication, access boundaries, content and health.
5. Attach the domain only after the checks pass and record the instance as ready.

Use explicit states: disabled, provisioning, ready, updating, suspended, failed, retiring. Retrying resumes recorded checkpoints and checks existing resources rather than creating duplicates. A failed job must not delete existing customer data. Disabling/suspending a CMS and permanently deleting its data are separate actions. Destructive deletion requires a distinct retention/export process.

Initially, checked-in CLI operations are sufficient; a queue/workflow engine is justified when activation becomes a UI action that must survive request timeouts. Do not run provisioning as one long browser request or silently execute schema mutations at server startup.

## Operator access and customer roles

Every CMS exposes the platform-owned `operator` account visibly. Customers cannot delete/change it, change its password, grant themselves this role or invoke system jobs. Customer administrators manage ordinary accounts within the installed policy; editors manage content; readers cannot write. The platform keeps recovery and deployment control outside the customer CMS.

No universal password, hidden account or arbitrary impersonation token is planned. A later “Open CMS as operator” flow should use a short-lived, single-use, audience-bound exchange with an audit trail and revocation. Until then, use the independent CMS login and existing operator credentials. Customer password recovery cannot modify an operator account unless the genuine operator recovery token is validated.

## Templates, collections and page building

Share a small, pinned policy/config package when repeated changes justify package publishing; keep deployments independently upgradeable. Share standard modules such as media, SEO and tracking while enabling only the modules a project uses. Project-specific collections stay in that project's template. Stall has no horse-profile collection.

The first management release can select installed modules and deploy a compatible template version. A browser collection designer is a separate later feature: accept a constrained schema description, validate names/relations/access, produce a versioned configuration and migration, review the proposed destructive effects, back up, then deploy. Never accept arbitrary executable JavaScript as schema configuration.

Existing Payload block/page editors remain available where already implemented. A cross-project page builder needs a versioned block registry and a rendering contract implemented by each frontend. Adding a block to the CMS alone must not imply every website can render it.

## Future customer panel

Reserve membership/ownership boundaries in the control-plane model now. Later the customer panel can show only authorized projects, domains, deployment status, CMS links and recovery actions. It must not expose operator credentials or direct infrastructure mutation APIs. Customer self-service, invitations, billing and a unified login are not part of this phase.

## Next increments

1. Use the selected Snowflake IDs to implement the small operator-only customer/project registry with optional CMS state.
2. Add explicit, resumable provisioning from one supported template and scoped secrets.
3. Add health, audit, backup/restore and controlled upgrades before broad self-service.
4. Implement the requested per-site Plausible tracking module for Spitzli, Webdock and Stall, with validated script configuration.
5. Add module selection and versioned block templates; consider a schema designer only after real use cases demonstrate its requirements.
