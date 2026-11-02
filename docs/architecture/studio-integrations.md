# Studio integrations

Studio is the operator workspace at https://studio.webdock.dev. Customer sites retain independent Payload applications and schema-scoped database credentials. Local Payload users are system-managed identity projections; native account screens and password authentication are disabled during enforced SSO.

## MCP

Connect an HTTP MCP client to `https://studio.webdock.dev/api/mcp`. The service implements stateless Streamable HTTP with JSON responses using the official TypeScript SDK. It does not offer a persistent SSE stream.

Register a client at https://auth.webdock.dev/connections while signed in as an operator with completed security setup. Supply the exact callback URL from the client. Public/native clients use PKCE without a secret; server clients can request a confidential client secret, displayed once. Dynamic anonymous registration is disabled. Every client requires user consent and an exact registered callback.

- `webdock:read`: `list_records`, `get_record` for customers, projects, CMS connections and activity.
- `webdock:write`: additionally `save_customer`, `save_project`, `save_cms_connection`, `set_archived`. Request both scopes for writes.

Writes share the Studio registry service and Payload access, validation, ownership and transaction-bound audit hooks. CMS connection changes edit inventory only. No tool deploys infrastructure, deletes content, reads credentials or manages identities. Snowflake IDs remain strings. Create operations are not idempotent; clients must not blindly retry after an ambiguous timeout.

The protected-resource discovery document is `/.well-known/oauth-protected-resource/api/mcp`. The issuer is `https://auth.webdock.dev/api/auth`; discovery is also available at its RFC 8414 path-insertion URL. Tokens must target the exact MCP resource. Native OAuth validation verifies PKCE, signature, audience linkage, expiration and current sessions; the resource introspection wrapper additionally reloads current operator/MFA/ban/setup status and registered MCP-client metadata. Disabling a client or signing out centrally invalidates access on the next request. Access tokens expire after five minutes. Clients explicitly registered for remote work can request `offline_access` for rotating refresh tokens; see the remote-work section below.

The existing Studio confidential SSO client is linked to the MCP resource as its introspector. It cannot use ordinary SSO tokens as MCP tokens: they have neither the resource audience nor MCP-client metadata. Never copy the introspection secret into an MCP client.

## GitHub

The Spitzli-owned **Webdock Studio** GitHub App uses only repository Metadata read access. It has no code write permissions or webhooks. Its exact callback is `https://studio.webdock.dev/api/github/callback`; the installation setup page returns to `/integrations`.

Studio requires its own operator login before GitHub connection. GitHub OAuth uses PKCE and encrypted, short-lived state bound to that operator. The expiring GitHub App user token is kept in an encrypted, HttpOnly, host-only cookie for at most eight hours; no refresh token is retained. It is never returned to frontend JavaScript. Connect again after expiration.

Repository selection reloads the authorized installation and repository list from GitHub before saving the verified canonical repository URL. Removing GitHub installation access therefore prevents subsequent selection. Existing repository URLs remain ordinary project inventory; disconnecting GitHub does not delete them.

Configuration uses production-only `WEBDOCK_GITHUB_CLIENT_ID`, `WEBDOCK_GITHUB_CLIENT_SECRET`, and `WEBDOCK_GITHUB_APP_SLUG`. Cookie encryption derives a separate key from the Studio SSO cookie secret. The GitHub App private key is not needed or deployed by this user-authorized integration.

## Passkeys

Passkeys are enrolled and removed at https://auth.webdock.dev/account and can initiate central sign-in. They are scoped to auth.webdock.dev. Device PIN/biometric verification is required on both registration and assertion. A verified passkey completes sign-in without an additional OTP challenge. Password sign-in still requires the configured MFA; recovery methods and the operator MFA-enrollment requirement remain available. Payload never stores these credentials.

## Sources

- [MCP authorization](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization)
- [MCP TypeScript SDK](https://github.com/modelcontextprotocol/typescript-sdk)
- [Better Auth OAuth provider](https://www.better-auth.com/docs/plugins/oauth-provider)
- [Better Auth passkeys](https://www.better-auth.com/docs/plugins/passkey)
- [GitHub App registration](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/registering-a-github-app)

## Remote work from T3 Code on Android

The agent and its MCP client run on the connected computer; the phone controls the conversation. An Android browser session is separate from the agent's desktop browser session. Use MCP/API for registry work and Playwright for unattended browser checks when the T3 preview host is unavailable. Complete any interactive sign-in, MFA, passkey ceremony or GitHub consent in the agent's shared browser at the PC. Never copy a personal password or MFA secret into a test script.

Register a client at the Connections page using the exact callback supplied by the MCP client. Enable registry changes if needed and **Allow automatic token renewal for remote work** for longer sessions. Request `webdock:read webdock:write offline_access` (omit write for read-only clients) and resource `https://studio.webdock.dev/api/mcp`. Authorization still requires PKCE and explicit consent. Existing registrations retain their original grants; register a new remote-work client rather than assuming an old client has refresh access.

Access tokens last five minutes. Opted-in clients receive rotating refresh tokens with an eight-hour lifetime. Store credentials in the MCP client's credential store on the host, replace the refresh token after every exchange, and serialize renewal requests: reuse of a rotated token is rejected. Refresh cannot expand permissions or resources. Central sign-out, account restrictions and disabling the client invalidate resource access; reconnect interactively when the session or refresh token expires. Disabling a client also prevents renewal.

Available remotely: customer/project creation, editing and archive/restore; deployed CMS inventory creation/editing; filtered and paginated reads; related records and audit activity. CMS provisioning, editing website content, identity administration and a persistent GitHub credential API are not included. GitHub account connection and verified repository selection continue through Studio's browser workflow.

### HTTP registry API

The API is another interface to the same Webdock registry resource and accepts the same OAuth bearer token and scopes as MCP. It never accepts a browser cookie as API authorization. An authenticated `GET /api/registry` returns operations, collections and JSON schemas; responses are not cached.

| Method | Path | Body/result |
| --- | --- | --- |
| GET | `/api/registry/{collection}` | Paginated records; `search`, `page`, `limit` (max 50), `status`, `sort` |
| GET | `/api/registry/{collection}/{id}` | One record |
| POST | `/api/registry/customers` | Customer fields; returns 201 |
| POST | `/api/registry/projects` | Project fields; returns 201 |
| POST | `/api/registry/cms-instances` | `{ "data": { ... }, "confirmExisting": true }`; records an existing CMS |
| PUT | `/api/registry/{collection}/{id}` | Same fields as create; updates the record |
| PATCH | `/api/registry/customers/{id}` or `/api/registry/projects/{id}` | `{ "archived": true }` or `{ "archived": false }` |

Collections are `customers`, `projects`, `cms-instances` and read-only `audit-events`. Projects support a `customer` ID filter; CMS connections support `project`; activity supports `targetCollection` and `targetID`. MCP `list_records` accepts the same filters. IDs are decimal strings. Unsupported filters, duplicate query parameters and unknown fields are rejected. Use JSON bodies (max 64 KiB). Deletion, identity changes and infrastructure operations are unavailable. Creates are not idempotent; after an ambiguous response, inspect the registry before retrying.

Example (the host client supplies `WEBDOCK_ACCESS_TOKEN`; never commit it):

```sh
curl --fail-with-body \
  -H "Authorization: Bearer $WEBDOCK_ACCESS_TOKEN" \
  'https://studio.webdock.dev/api/registry/projects?customer=123&status=active&sort=name'
```

### Upgrading an existing auth deployment

The OAuth provider seeds resources in insert-only mode, so adding `offline_access` to code does not update an existing resource policy. After deploying, run `bun --env-file=.env.instance scripts/enable-mcp-renewal.ts` from `apps/auth` using the intended environment. This idempotent update appends only `offline_access` to the existing enabled Webdock resource and leaves registered clients, their granted scopes and all other resource settings unchanged. No schema migration is required.

## Accounts, customer invitations and website access

Studio navigation provides **My account** and **People & access**, using the configured central auth issuer. Operators manage people at `/people` on that auth service; customer users cannot open or execute its administration actions. Account shortcuts and `/sites` let each customer discover only their own authorized content managers.

For an existing website, select the customer, enter the person's name/email, choose Reader, Editor or Administrator and send the invitation. New accounts receive a one-time password-setup email. Successful redemption proves mailbox ownership and completes initial password setup; sign-in returns to the pending invitation. The recipient must accept it before customer membership is active. Existing accounts receive a normal invitation. Existing customer-group members receive the new website grant immediately plus a notification email. Public sign-up remains closed.

Website roles do not grant Studio or platform access. Each CMS still introspects current account state, customer membership and the specific website grant. Revoking a grant or suspending an account removes effective access on the next protected request. Operators can restore customer accounts, change or revoke individual website roles, resend pending invitations and cancel invitations created by another operator. Cancelling a pending customer-group invitation also withdraws its pending website grants atomically, preventing a later invitation from activating abandoned access. Operator accounts cannot be suspended or assigned customer roles through this interface.

Customer groups here are identity/permission groups, not new Studio registry records. Existing website-to-group assignments are preserved; changing ownership requires a separate reviewed operation. This release does not create a customer self-service console, provision CMS instances or invite real customers automatically.

Before deploying the first build with `/people`, run `bun --env-file=.env.instance scripts/migrate-access.ts` from `apps/auth`. It adds only the `access_event` audit table. Actions record actor, operation, target and outcome, never passwords, tokens or email bodies. Native identity/email operations can partially complete if delivery fails; the UI tells the operator to review state and resend rather than claiming a rollback. Tests use local synthetic accounts and a captured mail outbox.

## CMS product interface

The customer product is **CMS**. Webdock, Spitzli and Stall serve a shared, mobile-friendly editor at `/cms`, backed by each website's independent content store. Legacy `/admin` links redirect there. The upstream technical interface moves to `/system` and requires a platform operator on pages, metadata and server functions. Customers manage their account through Webdock; roles/invitations remain in People & access rather than a local Users collection.

Content modules are explicitly allowlisted per site. Common fields, arrays, page blocks, media and relationships use the shared UI; Stall supplies a lossless rich-text adapter using its already-installed editor. Existing localized content, draft/public separation and native validation are retained. Saved-draft previews require instance authentication. Language editing does not imply a new public-language frontend: the Stall frontend still serves its existing default-language presentation.

Reader can view content. Editor can create, edit and publish. Administrator can additionally delete content and restore versions. These restrictions apply both to the CMS adapter and native content operations. Conflicting writes fail instead of overwriting another editor's changes. No content schema migration or live-content rewrite is needed for this rollout.

## Vercel live information

The private Vercel integration uses the classic connectable-account OAuth flow and requires exactly four scopes: `read:integration-configuration`, `read:deployment`, `read:domain`, and exactly one of `read:project` or `read-write:project`. Existing read-only installations remain supported. Vercel replaces `read:project` with `read-write:project` when project write permission is granted; it does not add a separate `write:project` scope. Both project scopes together, duplicate scopes and unrelated scopes are rejected. This code change does not upgrade an installation's grants. The ordinary central Webdock login remains unchanged.

The server-side `deleteVercelProject` adapter is reserved for the operator-authorized deletion service. It validates the exact project ID and owning team using GET before issuing the documented `DELETE /v9/projects/{id}?teamId={team}`. Only HTTP 204 means deletion succeeded. A 404 is accepted as already absent only when the caller supplies a matching previously verified target from its persisted journal; never construct that evidence from client input. HTTP 403 reports `deletion_forbidden`, explicitly identifying the missing project write permission, and never reports success. Provider text and credentials are not propagated. A read-only integration cannot perform this operation; an owner must explicitly approve the project permission upgrade in Vercel. No deployment, domain-editing or environment-variable operations are added.

References: [delete project endpoint](https://vercel.com/docs/rest-api/projects/delete-a-project), [integration permissions and scope upgrades](https://vercel.com/docs/integrations/create-integration/vercel-api-integrations).

Configure `WEBDOCK_VERCEL_CLIENT_ID`, `WEBDOCK_VERCEL_CLIENT_SECRET`, `WEBDOCK_VERCEL_INTEGRATION_SLUG`, `WEBDOCK_VERCEL_TEAM_ID`, and `WEBDOCK_VERCEL_TEAM_SLUG` (required for the private dashboard installation URL) on Studio. Its callback is `https://studio.webdock.dev/api/vercel/callback`. Before deployment run `bun --env-file=.env.instance scripts/migrate-vercel.ts` from `apps/admin`; this adds only the connection and project-reference tables.

Start installation from Studio → Integrations after signing in as an operator. The encrypted ten-minute flow cookie binds the callback to that operator, team, app and origin. Code exchange verifies the returned installation and its exact read scopes. The persistent access token is encrypted using a purpose-separated key derived from Studio's SSO cookie secret and is never returned to browser JavaScript, MCP or the registry API. Secret rotation therefore requires reconnecting. Installation revocation and denied provider access produce a visible connection error.

Project pages fetch current production assignment, the five latest deployments, Git metadata, framework/runtime and domains. Results display their check time, support manual refresh and are streamed separately from the normal registry view. Production is read from Vercel's actual production target, never inferred from the latest successful build. No secret/environment fields or raw provider error bodies are exposed.

Existing CMS `providerProjectID` references are used as a fallback. Explicit project links work independently of whether a project has a CMS and are verified against Vercel before saving. Connection/link mutations are committed together with immutable audit entries. Disconnecting deletes Studio's stored credential while preserving project links; revoke provider permission separately by removing the integration in Vercel.

The same status is available read-only through MCP `get_hosting_status` (`projectID`) and `GET /api/registry/projects/{id}/hosting`, with the registry's existing operator/scope checks. Provider fetches use fixed API origins, bounded responses and a finite timeout. A provider outage never grants extra access or turns a read into a mutation.

Production verification (2026-10-04): private Webdock Studio integration installed for Spitzli Development with the four read scopes above. The user selected All Projects; Studio lists 16 authorised Vercel projects. Live project panels verified for Webdock, Spitzli and Stall. Public integration documentation, terms and privacy notice are available under `https://webdock.dev/integrations/vercel`, `/terms` and `/privacy`. The installation starts at the team dashboard URL because private integrations return 404 through the public marketplace entry.

## Managed project deletion (October 6)

Platform operators can preview `/projects/{id}/delete`, use `GET /api/registry/projects/{id}/deletion` and `DELETE /api/registry/projects/{id}` with `{confirmName,planHash}`, or MCP `preview_project_deletion` / `delete_project`. Destructive REST/MCP operations require `webdock:write`; tenant admins are not platform operators. Native Auth validates the live operator and session independently before any provider deletion. Tenant-preview sessions cannot delete.

The shared deletion service validates unique CMS/hosting mappings, exact Vercel team/project/domain, OAuth website binding/customer/origin, and a dedicated isolated database runtime credential. `WEBDOCK_MANAGED_DATABASES` is a sensitive server-only map from schema to `{url}`; root/owner database credentials are rejected. Currently only isolated `demo_…` schemas with scoped runtime roles are supported. Run `scripts/migrate-project-deletion.ts` explicitly before enabling this feature. It creates an independent durable journal; no automatic production schema push.

Hosting, website identity and CMS schema are removed before registry rows. External failures retain registry rows and the journal. Repeating the same confirmed plan resumes completed work; provider/auth evidence also supports recovery when a response is lost. Changes to critical resource mappings block retries; ordinary notes/status/label edits do not. Renames require the current project name. Database dependencies into other schemas and elevated/shared runtime roles block deletion. Historical audit records, customer/tenant accounts and user accounts are retained. The local-only lead tracker and external mailbox drafts are not silently accessed by the hosted service.

A Vercel installation must include the target project and `read-write:project` to delete hosting. A personal CLI credential is never a runtime fallback. A missing grant is a blocking configuration issue, not permission to delete inventory only. Only the Gristeder demo has had its managed database credential configured so far; deletion of other websites needs their verified isolated mapping first.

### Callback completion correction

Token exchange and credential storage do not finish the Vercel installation transaction. After validating the signed flow, team, installation and exact scopes, the callback must redirect the popup to Vercel’s supplied `next` URL. The URL is required, bounded, and restricted to the exact `https://vercel.com` origin without credentials or fragments; invalid targets are rejected before token exchange. It is not stored with credentials. The callback clears the flow cookie and returns a 303 with no-store/no-referrer and `Cross-Origin-Opener-Policy: unsafe-none`. Returning directly to Studio previously displayed a premature success while the parent Vercel dialog subsequently cancelled the installation.

Official contract: https://vercel.com/docs/integrations/create-integration/submit-integration (Redirect URL / Completion). Tests cover both provider scope variants, callback binding and invalid completion targets. Diagnostic logs expose fixed failure stages and parameter-presence booleans only, never OAuth codes/tokens/cookies.
