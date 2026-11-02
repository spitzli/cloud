import { hostingCommandModule } from "@webdock/module-sdk/selection";
import {
  installationRegistry,
  requireHostingCommand,
} from "../../installation";
import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import {
  commandSchema,
  serviceCommands,
  setupCommands,
  byokCommands,
  gitCommands,
  databaseCommands,
  readActions,
  hostingError,
  type HostingCommand,
} from "@webdock/hosting-contracts";
export const hostingTools: Record<HostingCommand["action"], string> = {
  ...(Object.fromEntries(
    setupCommands.map((s) => [
      s.shape.action.value,
      s.shape.action.value.replaceAll(".", "_"),
    ]),
  ) as Record<
    (typeof setupCommands)[number]["shape"]["action"]["value"],
    string
  >),
  ...(Object.fromEntries(
    serviceCommands.map((s) => [
      s.shape.action.value,
      s.shape.action.value.replaceAll(".", "_"),
    ]),
  ) as Record<
    (typeof serviceCommands)[number]["shape"]["action"]["value"],
    string
  >),
  ...(Object.fromEntries(
    databaseCommands.map((s) => [
      s.shape.action.value,
      s.shape.action.value.replaceAll(".", "_"),
    ]),
  ) as Record<
    (typeof databaseCommands)[number]["shape"]["action"]["value"],
    string
  >),
  ...(Object.fromEntries(
    gitCommands.map((s) => [
      s.shape.action.value,
      s.shape.action.value.replaceAll(".", "_"),
    ]),
  ) as Record<
    (typeof gitCommands)[number]["shape"]["action"]["value"],
    string
  >),
  ...(Object.fromEntries(
    byokCommands.map((s) => [
      s.shape.action.value,
      s.shape.action.value.replaceAll(".", "_"),
    ]),
  ) as Record<
    (typeof byokCommands)[number]["shape"]["action"]["value"],
    string
  >),
  "clusters.activate": "activate_hosting_cluster",
  "apps.list": "list_hosting_apps",
  "apps.get": "get_hosting_app",
  "apps.prepare": "prepare_hosting_app",
  "apps.deploy": "deploy_prepared_hosting_app",
  "apps.create": "create_hosting_app",
  "apps.update": "update_hosting_app",
  "apps.scale": "scale_hosting_app",
  "apps.start": "start_hosting_app",
  "apps.stop": "stop_hosting_app",
  "apps.restart": "restart_hosting_app",
  "apps.rollback": "rollback_hosting_app",
  "apps.logs": "get_hosting_logs",
  "apps.deletion": "preview_hosting_app_deletion",
  "apps.delete": "delete_hosting_app",
  "apps.storageDeletion": "preview_hosting_storage_deletion",
  "apps.purgeStorage": "delete_hosting_storage",
  "apps.reconcile": "reconcile_hosting_app",

  "clusters.list": "list_hosting_clusters",
  "clusters.get": "get_hosting_cluster",
  "clusters.register": "register_hosting_cluster",
  "clusters.enrollment": "create_cluster_enrollment",
  "clusters.revoke": "revoke_hosting_agent",
  "limits.get": "get_hosting_limits",
  "limits.set": "set_hosting_limits",
  "usage.get": "get_hosting_usage",
  "projects.get": "get_hosting_project",
  "projects.overview": "get_hosting_project_overview",
  "projects.relocate": "relocate_hosting_project",
  "operations.overview": "get_hosting_overview",
  "projects.list": "list_hosting_projects",
  "projects.create": "create_hosting_project",
  "projects.update": "update_hosting_project",
  "operations.get": "get_hosting_operation",
};
export function registerHostingTools(
  server: McpServer,
  call: (cmd: HostingCommand) => Promise<unknown>,
  canWrite: boolean,
) {
  for (const schema of commandSchema.options) {
    if (schema.shape.action.value === "byok.mail.connect") continue;
    if (
      installationRegistry().state(
        hostingCommandModule(schema.shape.action.value),
      ) === "disabled"
    )
      continue;
    const action = schema.shape.action.value,
      read = readActions.has(action);
    if (!read && !canWrite) continue;
    const { action: actionSchema, ...shape } = schema.shape;
    void actionSchema;
    server.registerTool(
      hostingTools[action],
      {
        description: `${action}. Applies live customer permissions and hosting limits. Cluster enrollment returns a protected Studio setup reference, never a credential. Inventory registration does not install k3s. Application mutations enqueue quota-checked managed execution; inspect get_hosting_operation and get_hosting_app. Cached immutable images. Optional fixed-size persistent /data storage requires a verified storage agent and supports one replica. Application deletion retains data and its quota; purgeStorage separately destroys retained data. Deletion requires operator rights, exact name and current preview hash.`,
        inputSchema: shape,
        annotations: {
          readOnlyHint: read,
          destructiveHint:
            action === "projects.relocate" ||
            action === "databases.purgeMigrationSource" ||
            action === "databases.delete" ||
            action === "databases.purge" ||
            action === "clusters.revoke" ||
            action === "apps.delete" ||
            action === "apps.purgeStorage",
          idempotentHint:
            read ||
            action === "clusters.register" ||
            action === "projects.create" ||
            action === "projects.relocate" ||
            action.startsWith("apps.") ||
            action.startsWith("databases."),
          openWorldHint:
            action.startsWith("apps.") || action.startsWith("databases."),
        },
      },
      async (input: Record<string, unknown>) => {
        try {
          requireHostingCommand(action);
          return {
            content: [
              {
                type: "text" as const,
                text: JSON.stringify(
                  await call(commandSchema.parse({ ...input, action })),
                ),
              },
            ],
          };
        } catch (error) {
          return {
            isError: true,
            content: [
              { type: "text" as const, text: hostingError(error).message },
            ],
          };
        }
      },
    );
  }
}
export function validateHostingMCPToken(
  value: unknown,
  resource: string,
  issuer: string,
) {
  if (!value || typeof value !== "object") return null;
  const token = value as Record<string, unknown>;
  const aud = Array.isArray(token.aud) ? token.aud : [token.aud];
  if (
    token.active !== true ||
    token.disabled === true ||
    token.webdock_hosting !== true ||
    !["hosting-operator", "hosting-customer"].includes(
      String(token.webdock_role),
    ) ||
    token.iss !== issuer ||
    !aud.includes(resource) ||
    typeof token.sub !== "string" ||
    typeof token.sid !== "string" ||
    !token.sid ||
    typeof token.exp !== "number" ||
    !Number.isFinite(token.exp) ||
    token.exp <= Date.now() / 1000 ||
    typeof token.scope !== "string" ||
    token.token_type !== "Bearer"
  )
    return null;
  const scopes = new Set(token.scope.split(" "));
  if (
    !scopes.has("hosting:read") ||
    [...scopes].some((s) => s.startsWith("webdock:"))
  )
    return null;
  return { subject: token.sub, sessionID: token.sid, scopes };
}
