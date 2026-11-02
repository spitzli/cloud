import {
  createHash,
  createPrivateKey,
  createPublicKey,
  randomUUID,
  sign,
  timingSafeEqual,
} from "node:crypto";
import { readFileSync } from "node:fs";
import { z } from "zod";
import {
  currentDatabaseScope,
  withDatabaseScope,
  type DatabaseScope,
} from "@webdock/database-runtime";
import {
  PluginError,
  parsePluginManifest,
  type Capability,
  type PluginManifest,
} from "@webdock/plugin-sdk/contracts";
import { verifyInvocation } from "@webdock/plugin-sdk/server";
import {
  authorizePluginAction,
  resolvePluginConfiguration,
} from "@webdock/core/plugin-runtime-policy";
import { createPluginData } from "@webdock/core/plugin-data";
import {
  createPluginHTTP,
  pinnedHTTPTransport,
  type HTTPBinding,
} from "@webdock/core/plugin-http";
import {
  createPluginApps,
  dispatchManagedApps,
  type ManagedAppsHandler,
  type ManagedAppsProvisioningHandler,
  type ManagedAppBinding,
} from "@webdock/core/plugin-apps";
import { createPluginSessionRunner } from "./invocation-session";
import { revalidateStoredConsoleIdentity } from "../auth/clerk-provider";
import { withConsolePrincipal, storedConsoleIdentity } from "../auth/principal";
import { currentClaims } from "../auth/authorization";
import {
  authorizeWebsiteHTTPBinding,
  authorizePluginHTTPAction,
} from "./website-http";
const withPluginSession = createPluginSessionRunner({
  revalidate: revalidateStoredConsoleIdentity,
  withPrincipal: withConsolePrincipal,
});
import { database, workerDatabase } from "../../db/runtime";
const inputSchema = z.strictObject({
  pluginID: z.string().regex(/^[a-z][a-z0-9-]{0,63}$/),
  projectID: z.string().regex(/^[0-9]{1,20}$/),
  action: z.string().min(1).max(64),
  input: z.unknown(),
  idempotencyKey: z.string().min(1).max(128),
});
const controller = <T>(fn: () => Promise<T>) =>
  withDatabaseScope({ kind: "system", purpose: "plugin-controller" }, fn);
// Operator-only deployment file, mounted as a Secret. No URLs/credentials from invocation payloads.
type ScopedHTTPBinding = HTTPBinding & { websiteBindingID?: string };
type ResourceScope = {
  pluginID: string;
  customerID: string;
  projectID: string;
};
type ResourceBindings = {
  http(scope: ResourceScope, name: string): Promise<ScopedHTTPBinding | null>;
  apps(scope: ResourceScope): Promise<ManagedAppBinding[]>;
};
let resourceBindings: ResourceBindings | undefined;
/** Optional product adapter; Core owns no application template or provider credentials. */
export function configurePluginResourceBindings(bindings: ResourceBindings) {
  resourceBindings = bindings;
}
type RuntimeBinding = {
  uiOrigin?: string;
  publicConfiguration?: Record<string, unknown>;
  generation: number;
  namespace: string;
  service: string;
  addresses: string[];
  credential: string;
  ca: string;
  projects: Record<string, Record<string, ScopedHTTPBinding>>;
  managedApps?: Record<string, ManagedAppBinding[]>;
};
type Settings = {
  privateKey: string;
  hostOrigin: string;
  runtimes: Record<string, RuntimeBinding>;
};
function settings(): Settings {
  const file = process.env.WEBDOCK_PLUGIN_RUNTIME_FILE;
  if (!file)
    throw new PluginError(
      "capability_unavailable",
      "Plugin runtime is not configured",
    );
  const value = JSON.parse(readFileSync(file, "utf8")) as Settings;
  const origin = new URL(value.hostOrigin);
  if (
    origin.protocol !== "https:" ||
    origin.username ||
    origin.password ||
    origin.pathname !== "/" ||
    origin.search ||
    origin.hash
  )
    throw new PluginError("unavailable", "Invalid plugin host configuration");
  return value;
}
const publicKey = (s: Settings) =>
  createPublicKey(createPrivateKey(s.privateKey))
    .export({ type: "spki", format: "pem" })
    .toString();
function currentSession() {
  const scope = currentDatabaseScope();
  if (scope?.kind !== "session")
    throw new PluginError("permission_denied", "Verified session required");
  return scope;
}
async function projectAccess(projectID: string) {
  const row = (
    await database.query(
      `SELECT p.id,p.customer_id AS "customerID",p.status='active' AND c.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted') AS active,webdock_rls.customer_allowed(p.customer_id,false) AS allowed,webdock_rls.customer_allowed(p.customer_id,true) AS "canWrite",webdock_rls.actor()->'preview'<>'null'::jsonb AS preview FROM webdock_admin.projects p JOIN webdock_admin.customers c ON c.id=p.customer_id WHERE p.id=$1`,
      [projectID],
    )
  ).rows[0];
  if (!row) throw new PluginError("permission_denied", "Project unavailable");
  return row;
}
async function installation(pluginID: string, projectID?: string) {
  return controller(async () => {
    const row = (
      await workerDatabase.query(
        `SELECT p.*,r.generation AS runtime_generation,r.status AS runtime_status,r.configuration_revision,r.namespace,r.service_name,r.credential_hash,c.revision AS config_revision,c.approved_capabilities,c.configuration,l.manifest_bytes,l.manifest_sha256,l.image AS release_image FROM webdock_auth.plugin_installation p JOIN webdock_auth.plugin_runtime r ON r.plugin_id=p.id AND r.generation=p.current_generation JOIN webdock_auth.plugin_configuration c ON c.plugin_id=p.id JOIN webdock_auth.plugin_release l ON l.plugin_id=p.id AND l.version=p.version WHERE p.id=$1 AND p.kind='external'`,
        [pluginID],
      )
    ).rows[0];
    if (!row) throw new PluginError("not_found", "Plugin is not installed");
    if (
      createHash("sha256").update(row.manifest_bytes).digest("hex") !==
      row.manifest_sha256
    )
      throw new PluginError(
        "unavailable",
        "Installed manifest integrity failed",
      );
    const manifest = parsePluginManifest(JSON.parse(row.manifest_bytes));
    if (
      manifest.name !== pluginID ||
      manifest.version !== row.version ||
      manifest.image !== row.release_image
    )
      throw new PluginError(
        "unavailable",
        "Installed release identity mismatch",
      );
    const grant = projectID
      ? (
          await workerDatabase.query(
            "SELECT * FROM webdock_auth.plugin_grant WHERE plugin_id=$1 AND project_id=$2",
            [pluginID, projectID],
          )
        ).rows[0]
      : null;
    return { row, manifest, grant };
  });
}
export async function listRuntimePlugins(projectID?: string) {
  const session = currentSession();
  const project = projectID ? await projectAccess(projectID) : undefined;
  const ids = await controller(
    async () =>
      (
        await workerDatabase.query(
          "SELECT id FROM webdock_auth.plugin_installation WHERE kind='external' AND state='enabled' AND runtime='ready' ORDER BY id",
        )
      ).rows,
  );
  const result = [];
  for (const { id } of ids) {
    const { row, manifest, grant } = await installation(id, projectID);
    if (projectID && (!grant || grant.revoked_at)) continue;
    let launchLinks: Array<{
      appID: string;
      label: string;
      appURL: string;
      consolePath: string;
    }> = [];
    if (projectID && project && managedAppsHandler && declaresApps(manifest)) {
      const action = manifest.actions.find(
        (a) =>
          a.intent === "read" && a.capabilities.includes("plugin.apps.read"),
      );
      if (action)
        try {
          authorizePluginAction(
            {
              state: row.state,
              runtime: row.runtime_status === "ready" ? row.runtime : "failed",
              generation: Number(row.current_generation),
              runtimeGeneration: Number(row.runtime_generation),
              configurationRevision: Number(row.config_revision),
              runtimeConfigurationRevision: Number(row.configuration_revision),
              grantRevision: Number(grant?.revision ?? 0),
              revoked:
                !grant ||
                !!grant.revoked_at ||
                grant.customer_id !== project.customerID,
              approved: row.approved_capabilities,
              granted: grant?.capabilities ?? [],
              actions: manifest.actions,
              project,
            },
            action.name,
          );
          const binding = runtimeBinding(settings(), id, row);
          const listed = await dispatchManagedApps(
            {
              pluginID: id,
              projectID,
              customerID: project.customerID,
              operationID: "",
              subject: session.subject,
              sessionID: session.sessionID,
            },
            [
              ...(binding.managedApps?.[projectID] ?? []),
              ...((await resourceBindings?.apps({
                pluginID: id,
                projectID,
                customerID: project.customerID,
              })) ?? []),
            ],
            { operation: "list", input: {} },
            managedAppsHandler,
          );
          if ("apps" in listed)
            launchLinks = listed.apps.map((app) => ({
              appID: app.app_id,
              label: app.name,
              appURL: app.origin,
              consolePath: app.console_path,
            }));
        } catch {
          /* Unavailable or unauthorized apps expose no launch links. */
        }
    }
    result.push({
      launchLinks,
      state: row.state,
      runtime: row.runtime,
      manifest,
      uiOrigin: settings().runtimes[id]?.uiOrigin,
    });
  }
  return result;
}
export async function getRuntimePlugin(pluginID: string) {
  const { row, manifest } = await installation(pluginID);
  return { state: row.state, runtime: row.runtime, manifest };
}
function runtimeBinding(s: Settings, id: string, row: any) {
  const b = s.runtimes[id];
  if (
    !b ||
    b.generation !== Number(row.current_generation) ||
    b.namespace !== row.namespace ||
    b.service !== row.service_name ||
    !/^[a-z0-9-]+$/.test(b.namespace) ||
    !/^[a-z0-9-]+$/.test(b.service) ||
    createHash("sha256").update(b.credential).digest("hex") !==
      row.credential_hash
  )
    throw new PluginError("unavailable", "Runtime binding mismatch");
  return b;
}
function validateValue(
  schema: PluginManifest["configuration"] | undefined,
  value: unknown,
) {
  if (schema && !z.fromJSONSchema(schema as any).safeParse(value).success)
    throw new PluginError(
      "invalid_input",
      "Action payload does not match its declared schema",
    );
  if (Buffer.byteLength(JSON.stringify(value) ?? "null") > 32768)
    throw new PluginError("invalid_input", "Action payload exceeds limit");
}
export async function invokeRuntimePlugin(
  raw: unknown,
  ceiling: { canWrite: boolean } = { canWrite: true },
) {
  const input = inputSchema.parse(raw);
  const scope = currentSession();
  const project = await projectAccess(input.projectID);
  const { row, manifest, grant } = await installation(
    input.pluginID,
    input.projectID,
  );
  const action = authorizePluginAction(
    {
      state: row.state,
      runtime: row.runtime_status === "ready" ? row.runtime : "failed",
      generation: Number(row.current_generation),
      runtimeGeneration: Number(row.runtime_generation),
      configurationRevision: Number(row.config_revision),
      runtimeConfigurationRevision: Number(row.configuration_revision),
      grantRevision: Number(grant?.revision ?? 0),
      revoked:
        !grant ||
        !!grant.revoked_at ||
        grant.customer_id !== project.customerID,
      approved: row.approved_capabilities,
      granted: grant?.capabilities ?? [],
      actions: manifest.actions,
      project,
    },
    input.action,
    { deferWebsiteHTTP: true },
  );
  if (action.intent === "write" && !ceiling.canWrite)
    throw new PluginError("permission_denied", "Write scope required");
  validateValue(action.input, input.input);
  const config = settings();
  const binding = runtimeBinding(config, input.pluginID, row);
  const now = Math.floor(Date.now() / 1000);
  const claims = {
    v: 1 as const,
    id: randomUUID(),
    pluginID: input.pluginID,
    generation: Number(row.current_generation),
    action: action.name,
    projectID: input.projectID,
    iat: now,
    exp: now + 60,
  };
  const requestHash = createHash("sha256")
    .update(JSON.stringify(input.input))
    .digest("hex");
  const operationID = createHash("sha256")
    .update(
      JSON.stringify([
        scope.subject,
        input.pluginID,
        input.projectID,
        input.action,
        input.idempotencyKey,
      ]),
    )
    .digest("hex");
  await controller(async () => {
    const c = await workerDatabase.connect();
    try {
      await c.query("BEGIN");
      await c.query(
        "SELECT pg_advisory_xact_lock_shared(hashtext('webdock-plugin-installation'))",
      );
      await c.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
        operationID,
      ]);
      const prior = (
        await c.query(
          "SELECT request_hash FROM webdock_auth.plugin_invocation WHERE plugin_id=$1 AND project_id=$2 AND actor_id=$3 AND operation_id=$4 LIMIT 1",
          [input.pluginID, input.projectID, scope.subject, operationID],
        )
      ).rows[0];
      if (prior && prior.request_hash !== requestHash)
        throw new PluginError(
          "conflict",
          "Idempotency key reused with different input",
        );
      const live = (
        await c.query(
          `SELECT 1 FROM webdock_auth.plugin_installation p JOIN webdock_auth.plugin_configuration c ON c.plugin_id=p.id JOIN webdock_auth.plugin_grant g ON g.plugin_id=p.id WHERE p.id=$1 AND p.state='enabled' AND p.current_generation=$2 AND g.project_id=$3 AND g.customer_id=$4 AND g.revision=$5 AND g.revoked_at IS NULL AND c.revision=$6`,
          [
            input.pluginID,
            claims.generation,
            input.projectID,
            project.customerID,
            grant.revision,
            row.config_revision,
          ],
        )
      ).rows.length;
      if (!live)
        throw new PluginError("conflict", "Plugin changed during admission");
      await c.query(
        `INSERT INTO webdock_auth.plugin_invocation(id,plugin_id,customer_id,project_id,runtime_generation,grant_revision,kind,name,intent,actor_id,session_id,operation_id,capabilities,expires_at,session_context,request_hash) VALUES($1,$2,$3,$4,$5,$6,'action',$7,$8,$9,$10,$11,$12::jsonb,to_timestamp($13),$14::jsonb,$15)`,
        [
          claims.id,
          input.pluginID,
          project.customerID,
          input.projectID,
          claims.generation,
          grant.revision,
          action.name,
          action.intent,
          scope.subject,
          scope.sessionID,
          operationID,
          JSON.stringify(action.capabilities),
          claims.exp,
          JSON.stringify(scope),
          requestHash,
        ],
      );
      await c.query("COMMIT");
    } catch (e) {
      await c.query("ROLLBACK");
      throw e;
    } finally {
      c.release();
    }
  });
  const body = Buffer.from(JSON.stringify(claims)).toString("base64url");
  const token =
    body +
    "." +
    sign(null, Buffer.from(body), config.privateKey).toString("base64url");
  try {
    // No runtime sees this invocation until its exact website authority is checked.
    await withInvocation(
      new Request(config.hostOrigin, {
        headers: {
          authorization: `Bearer ${token}`,
          "x-plugin-runtime": binding.credential,
        },
      }),
      undefined,
      async () => null,
    );
    const result = await pinnedHTTPTransport({
      url: new URL(
        `https://${binding.service}.${binding.namespace}.svc.cluster.local:3443/actions/${encodeURIComponent(action.name)}`,
      ),
      addresses: binding.addresses,
      ca: binding.ca,
      method: "POST",
      headers: {
        authorization: `Bearer ${binding.credential}`,
        "x-plugin-invocation": token,
      },
      body: JSON.stringify({
        input: input.input,
        configuration: resolvePluginConfiguration(
          manifest.configuration,
          row.configuration,
          binding.publicConfiguration,
          input.projectID,
        ),
        hostOrigin: config.hostOrigin,
      }),
    });
    // Refuse a late result after revocation, identity expiry or project reassignment.
    await withInvocation(
      new Request(config.hostOrigin, {
        headers: {
          authorization: `Bearer ${token}`,
          "x-plugin-runtime": binding.credential,
        },
      }),
      undefined,
      async () => null,
    );
    if (result.status !== 200)
      throw new PluginError(
        "unavailable",
        "Plugin action failed",
        result.status >= 500,
      );
    validateValue(action.output, result.body);
    return result.body;
  } finally {
    await controller(async () => {
      await workerDatabase.query(
        "UPDATE webdock_auth.plugin_invocation SET revoked_at=now() WHERE id=$1",
        [claims.id],
      );
    });
  }
}
async function withInvocation<T>(
  request: Request,
  capability: Capability | undefined,
  fn: (
    scope: {
      pluginID: string;
      customerID: string;
      projectID: string;
      operationID: string;
    },
    binding: RuntimeBinding,
    websiteBinding?: HTTPBinding,
  ) => Promise<T>,
  httpRequest?: { name: string; method: string },
): Promise<T> {
  const config = settings();
  const token =
    request.headers.get("authorization")?.replace(/^Bearer /, "") ?? "";
  // Decode only to select an operator-owned verifier target; verify before reading metadata.
  let untrusted: any;
  try {
    untrusted = JSON.parse(
      Buffer.from(token.split(".")[0], "base64url").toString(),
    );
  } catch {
    throw new PluginError("permission_denied", "Invalid invocation");
  }
  const candidate = config.runtimes[untrusted.pluginID];
  if (!candidate) throw new PluginError("permission_denied", "Unknown runtime");
  const claims = verifyInvocation(token, {
    publicKey: publicKey(config),
    pluginID: untrusted.pluginID,
    generation: candidate.generation,
  });
  const received = Buffer.from(request.headers.get("x-plugin-runtime") ?? "");
  const expected = Buffer.from(candidate.credential);
  if (
    received.length !== expected.length ||
    !timingSafeEqual(received, expected)
  )
    throw new PluginError("permission_denied", "Runtime identity required");
  const inv = await controller(
    async () =>
      (
        await workerDatabase.query(
          "SELECT * FROM webdock_auth.plugin_invocation WHERE id=$1 AND revoked_at IS NULL AND expires_at>now()",
          [claims.id],
        )
      ).rows[0],
  );
  if (
    !inv ||
    inv.plugin_id !== claims.pluginID ||
    inv.project_id !== claims.projectID ||
    inv.name !== claims.action ||
    Number(inv.runtime_generation) !== claims.generation
  )
    throw new PluginError("permission_denied", "Invocation unavailable");
  const state = await installation(claims.pluginID, claims.projectID);
  runtimeBinding(config, claims.pluginID, state.row);
  const session = inv.session_context as DatabaseScope;
  if (
    session.kind !== "session" ||
    session.subject !== inv.actor_id ||
    session.sessionID !== inv.session_id
  )
    throw new PluginError(
      "permission_denied",
      "Invocation identity unavailable",
    );
  return withPluginSession(session, claims.id, async () => {
    const scoped = (
      await database.query("SELECT webdock_rls.plugin_context($1) AS scope", [
        capability,
      ])
    ).rows[0]?.scope;
    if (!scoped)
      throw new PluginError("permission_denied", "Plugin capability revoked");
    const access = await projectAccess(scoped.projectID);
    const action = state.manifest.actions.find(
      (candidate) => candidate.name === inv.name,
    );
    if (!action)
      throw new PluginError("permission_denied", "Invocation action changed");
    if (action.httpBinding)
      authorizePluginAction(
        {
          state: state.row.state,
          runtime:
            state.row.runtime_status === "ready" ? state.row.runtime : "failed",
          generation: Number(state.row.current_generation),
          runtimeGeneration: Number(state.row.runtime_generation),
          configurationRevision: Number(state.row.config_revision),
          runtimeConfigurationRevision: Number(
            state.row.configuration_revision,
          ),
          grantRevision: Number(state.grant?.revision ?? 0),
          revoked:
            !state.grant ||
            !!state.grant.revoked_at ||
            state.grant.customer_id !== scoped.customerID,
          approved: state.row.approved_capabilities,
          granted: state.grant?.capabilities ?? [],
          actions: state.manifest.actions,
          project: access,
        },
        inv.name,
        { deferWebsiteHTTP: true },
      );
    if (
      action.intent !== inv.intent ||
      JSON.stringify(action.capabilities) !== JSON.stringify(inv.capabilities)
    )
      throw new PluginError("permission_denied", "Invocation action changed");
    const authorized = await authorizePluginHTTPAction(
      {
        subject: session.subject,
        customerID: scoped.customerID,
        projectID: scoped.projectID,
        generation: candidate.generation,
        grantRevision: Number(state.grant.revision),
      },
      action,
      access,
      {
        ...websiteAuthority,
        resolve: async (name) =>
          candidate.projects[scoped.projectID]?.[name] ??
          (await resourceBindings?.http(scoped, name)) ??
          null,
      },
      { capability, http: httpRequest },
    );
    return fn(scoped, candidate, authorized);
  });
}
export const pluginData = createPluginData({
  run: (req, cap, work) => withInvocation(req, cap, (s) => work(s, database)),
});
const websiteAuthority = {
  claims: currentClaims,
  website: async (id: string) =>
    (
      await database.query(
        `SELECT b.enabled AND NOT c.disabled AS enabled,t.customer_id AS "customerID",c."redirectUris" AS "redirectURIs"
   FROM webdock_auth.app_binding b JOIN webdock_auth."oauthClient" c ON c."clientId"=b.client_id
   JOIN webdock_auth.tenant_customer t ON t.organization_id=b.organization_id WHERE b.id=$1`,
        [id],
      )
    ).rows[0] ?? null,
};
export const pluginHTTP = createPluginHTTP({
  authorize: (req, name, method) =>
    withInvocation(
      req,
      method === "GET" ? "plugin.http.read" : "plugin.http.request",
      async (s, b, authorized) => {
        if (authorized) return authorized;
        const installed = await installation(s.pluginID, s.projectID);
        if (!installed.manifest.httpBindings.includes(name))
          throw new PluginError("permission_denied", "Undeclared HTTP binding");
        const binding =
          b.projects[s.projectID]?.[name] ??
          (await resourceBindings?.http(s, name));
        if (!binding)
          throw new PluginError(
            "capability_unavailable",
            "HTTP binding is not configured for this project",
          );
        if (!binding.websiteBindingID) return binding;
        const session = currentSession();
        return authorizeWebsiteHTTPBinding(
          {
            subject: session.subject,
            customerID: s.customerID,
            projectID: s.projectID,
            generation: b.generation,
            grantRevision: Number(installed.grant?.revision),
          },
          binding.websiteBindingID,
          binding,
          method,
          websiteAuthority,
        );
      },
      { name, method },
    ),
});

let managedAppsHandler: ManagedAppsHandler | undefined;
let managedAppsProvisioningHandler: ManagedAppsProvisioningHandler | undefined;
export function configureManagedApps(handler: ManagedAppsHandler) {
  managedAppsHandler = handler;
}
export function configureManagedAppProvisioning(
  handler: ManagedAppsProvisioningHandler,
) {
  managedAppsProvisioningHandler = handler;
}
function declaresApps(manifest: PluginManifest) {
  return [...manifest.services.required, ...manifest.services.optional].some(
    (service) => service.name === "apps",
  );
}
export const pluginApps = createPluginApps({
  handler: () => managedAppsHandler,
  provisioningHandler: () => managedAppsProvisioningHandler,
  run: (request, capability, work) =>
    withInvocation(request, capability, async (scope, binding) => {
      const installed = await installation(scope.pluginID, scope.projectID);
      if (!declaresApps(installed.manifest))
        throw new PluginError("permission_denied", "Undeclared apps service");
      const session = currentSession();
      return work(
        {
          ...scope,
          subject: session.subject,
          sessionID: session.sessionID,
          pluginGeneration: binding.generation,
          grantRevision: Number(installed.grant?.revision),
          consoleIdentity: storedConsoleIdentity() ?? undefined,
        },
        [
          ...(binding.managedApps?.[scope.projectID] ?? []),
          ...((await resourceBindings?.apps(scope)) ?? []),
        ],
      );
    }),
});
