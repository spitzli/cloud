import type { HostingPorts } from "./ports";
import { createHash } from "node:crypto";
import { HostingError } from "@spitzli/hosting-contracts";
import type { Connection } from "./authorization";
export type Dependencies = {
  decryptMailSecret: HostingPorts["decryptMailSecret"];
  vercelJSON: ReturnType<
    typeof import("./byok-vercel").createHostingByokVercel
  >["vercelJSON"];
  env: HostingPorts["env"];
};
// These credentials are consumed only by trusted preparation/publication, never by lease JSON.
export type GitVercelCredential = {
  token: string;
  teamID: string;
  configurationID: string;
  projectID: string;
  generation: number;
};
type ProviderJSON = (
  path: string,
  token: string,
  teamID: string,
) => Promise<Record<string, unknown>>;
type Decrypt = (
  cipher: string,
  purpose: string,
) => {
  token: string;
  scopes: string[];
};
export const gitVercelSchemaSQL = `
CREATE UNIQUE INDEX IF NOT EXISTS hosting_project_customer_key ON webdock_auth.hosting_project(project_id,customer_id);
CREATE TABLE IF NOT EXISTS webdock_auth.git_vercel_target (
 customer_id varchar NOT NULL, project_id varchar NOT NULL, target_id text NOT NULL CHECK(target_id ~ '^prj_[A-Za-z0-9]+$'),
 team_id text NOT NULL CHECK(team_id ~ '^team_[A-Za-z0-9]+$'), mode text NOT NULL CHECK(mode IN ('byok','platform')),
 generation integer NOT NULL DEFAULT 1 CHECK(generation>0),
 PRIMARY KEY(project_id,target_id), FOREIGN KEY(project_id,customer_id) REFERENCES webdock_auth.hosting_project(project_id,customer_id) ON DELETE RESTRICT,
 UNIQUE(team_id,target_id)
);`;
export function createHostingGitVercel(dependencies: Dependencies) {
  const missing = (message: string) => new HostingError(409, message);
  const denied = () =>
    new HostingError(403, "Vercel deployment target could not be verified.");
  const providerJSON: ProviderJSON = async (path, token, teamID) =>
    dependencies.vercelJSON(path, token, teamID);
  async function resolveGitVercelCredential(
    db: Connection,
    target: {
      customerID: string;
      projectID?: string;
      targetID: string;
      mode?: "byok" | "platform";
    },
    decrypt?: Decrypt,
  ): Promise<GitVercelCredential> {
    if (
      !/^[1-9][0-9]{0,19}$/.test(target.customerID) ||
      !/^prj_[A-Za-z0-9]+$/.test(target.targetID) ||
      (target.projectID && !/^[1-9][0-9]{0,19}$/.test(target.projectID))
    )
      throw denied();
    const binding = target.projectID
      ? (
          await db.query(
            "SELECT t.* FROM webdock_auth.git_vercel_target t JOIN webdock_auth.hosting_project p ON p.project_id=t.project_id AND p.customer_id=t.customer_id WHERE t.customer_id=$1 AND t.project_id=$2 AND t.target_id=$3 AND p.provider='vercel'",
            [target.customerID, target.projectID, target.targetID],
          )
        ).rows[0]
      : undefined;
    if (
      target.projectID &&
      (!binding ||
        binding.customer_id !== target.customerID ||
        binding.project_id !== target.projectID ||
        binding.target_id !== target.targetID)
    )
      throw denied();
    const mode = binding?.mode ?? target.mode;
    if (target.mode && binding && target.mode !== binding.mode) throw denied();
    if (mode === "platform") {
      const token = dependencies.env.SPITZLI_GIT_VERCEL_TOKEN,
        teamID = dependencies.env.SPITZLI_GIT_VERCEL_TEAM_ID,
        configurationID = dependencies.env.SPITZLI_GIT_VERCEL_CONFIGURATION_ID;
      if (
        !binding ||
        !token ||
        token.length > 8192 ||
        binding.team_id !== teamID ||
        !/^team_[A-Za-z0-9]+$/.test(teamID ?? "") ||
        !/^icfg_[A-Za-z0-9]+$/.test(configurationID ?? "") ||
        !Number.isSafeInteger(binding.generation) ||
        binding.generation < 1
      )
        throw missing(
          "Configure an explicitly bound platform Vercel deployment credential.",
        );
      return {
        token,
        teamID: teamID!,
        configurationID: configurationID!,
        projectID: target.targetID,
        generation: binding.generation,
      };
    }
    if (mode !== "byok")
      throw missing(
        "Select a Vercel deployment target before enabling Git deployments.",
      );
    const row = (
      await db.query(
        "SELECT customer_id,team_id,configuration_id,projects,revision,encrypted_token FROM webdock_auth.hosting_vercel WHERE customer_id=$1",
        [target.customerID],
      )
    ).rows[0];
    if (
      !row ||
      row.customer_id !== target.customerID ||
      !Array.isArray(row.projects) ||
      !row.projects.includes(target.targetID) ||
      (binding && row.team_id !== binding.team_id)
    )
      throw missing(
        "Select a Vercel connection and project before enabling Git deployments.",
      );
    const decode =
      decrypt ??
      dependencies.decryptMailSecret<{
        token: string;
        scopes: string[];
      }>;
    const token = decode(
      row.encrypted_token,
      "byok-vercel:" + target.customerID + ":" + row.team_id,
    ).token;
    if (
      typeof token !== "string" ||
      !token ||
      token.length > 8192 ||
      !/^team_[A-Za-z0-9]+$/.test(row.team_id) ||
      !/^icfg_[A-Za-z0-9]+$/.test(row.configuration_id) ||
      !Number.isSafeInteger(row.revision) ||
      row.revision < 1
    )
      throw denied();
    return {
      token,
      teamID: row.team_id,
      configurationID: row.configuration_id,
      projectID: target.targetID,
      generation: row.revision,
    };
  }
  async function prepareGitVercelBuild(
    credential: GitVercelCredential,
    input: {
      buildEnvironment: Record<string, string>;
      allowedBuildVariables: string[];
    },
    json: ProviderJSON = providerJSON,
  ) {
    if (
      !/^team_[A-Za-z0-9]+$/.test(credential.teamID) ||
      !/^prj_[A-Za-z0-9]+$/.test(credential.projectID) ||
      !/^icfg_[A-Za-z0-9]+$/.test(credential.configurationID)
    )
      throw denied();
    const buildEnvironment: Record<string, string> = {};
    let bytes = 0;
    for (const [key, value] of Object.entries(input.buildEnvironment)) {
      if (
        !/^[A-Z_][A-Z0-9_]{0,127}$/.test(key) ||
        /^(VERCEL|SPITZLI|WEBDOCK|GITHUB|AWS|KUBERNETES|DOCKER|REGISTRY)_/.test(key) ||
        [
          "NODE_OPTIONS",
          "PATH",
          "HOME",
          "LD_PRELOAD",
          "LD_LIBRARY_PATH",
          "BASH_ENV",
          "ENV",
        ].includes(key) ||
        !input.allowedBuildVariables.includes(key) ||
        typeof value !== "string" ||
        value.includes("\0") ||
        Buffer.byteLength(value) > 32768
      )
        throw missing(
          "Build variables must be explicitly approved and cannot contain provider credentials.",
        );
      bytes += Buffer.byteLength(key) + Buffer.byteLength(value);
      if (bytes > 65536 || Object.keys(buildEnvironment).length >= 128)
        throw missing("Build environment exceeds the allowed size.");
      buildEnvironment[key] = value;
    }
    const configuration = await json(
      "/v1/integrations/configuration/" + credential.configurationID,
      credential.token,
      credential.teamID,
    );
    if (
      configuration.id !== credential.configurationID ||
      configuration.teamId !== credential.teamID ||
      configuration.disabledAt ||
      configuration.deletedAt ||
      configuration.deleteRequestedAt ||
      !Array.isArray(configuration.scopes) ||
      !configuration.scopes.includes("read-write:deployment") ||
      !configuration.scopes.some((s: string) =>
        ["read:project", "read-write:project"].includes(s),
      ) ||
      (Array.isArray(configuration.projects) &&
        !configuration.projects.includes(credential.projectID))
    )
      throw missing(
        "Reconnect Vercel with deployment write and project read permissions.",
      );
    const project = await json(
      "/v9/projects/" + credential.projectID,
      credential.token,
      credential.teamID,
    );
    if (
      project.id !== credential.projectID ||
      project.accountId !== credential.teamID
    )
      throw denied();
    if (project.link)
      throw missing(
        "Disconnect native Vercel Git builds before enabling Webdock publication.",
      );
    const resourceConfig = (project.resourceConfig ?? project) as Record<
      string,
      unknown
    >;
    if (
      !Array.isArray(resourceConfig.functionDefaultRegions) ||
      resourceConfig.functionDefaultRegions.length !== 1 ||
      resourceConfig.functionDefaultRegions[0] !== "fra1" ||
      resourceConfig.functionZeroConfigFailover !== false
    )
      throw missing(
        "Configure Frankfurt Functions and disable cross-region failover before publication.",
      );
    const settings: Record<string, string | null> = {};
    for (const key of [
      "framework",
      "buildCommand",
      "outputDirectory",
      "installCommand",
      "devCommand",
      "nodeVersion",
      "rootDirectory",
    ]) {
      const value = project[key] ?? null;
      if (
        value !== null &&
        (typeof value !== "string" ||
          value.length > 4096 ||
          value.includes("\0"))
      )
        throw denied();
      settings[key] = value;
    }
    if (settings.nodeVersion !== "24.x")
      throw missing("Configure Node.js 24 for this Vercel build target.");
    if (
      settings.rootDirectory &&
      (settings.rootDirectory.startsWith("/") ||
        settings.rootDirectory.includes("\\") ||
        settings.rootDirectory
          .split("/")
          .some((part) => !part || part === "." || part === ".."))
    )
      throw denied();
    const vercelSettings = {
      orgId: credential.teamID,
      projectId: credential.projectID,
      settings,
    };
    return {
      vercelSettings,
      buildEnvironment,
      environment: "production" as const,
      connectionGeneration: credential.generation,
      configurationChecksum: createHash("sha256")
        .update(JSON.stringify({ vercelSettings, buildEnvironment }))
        .digest("hex"),
    };
  }
  return { resolveGitVercelCredential, prepareGitVercelBuild };
}
