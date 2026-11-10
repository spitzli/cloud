import type { HostingPorts } from "./ports";
import { createHash, randomBytes } from "node:crypto";
import { HostingError, type HostingActor } from "@spitzli/hosting-contracts";
import {
  gitOAuthCommandSchema,
  type GitOAuthCommand,
} from "@spitzli/hosting-contracts/git-oauth";
export type Dependencies = {
  withHostingDatabase: HostingPorts["withHostingDatabase"];
  encryptMailSecret: HostingPorts["encryptMailSecret"];
  decryptMailSecret: HostingPorts["decryptMailSecret"];
  authorizeHosting: ReturnType<
    typeof import("./authorization").createHostingAuthorization
  >["authorizeHosting"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  createGitHubProvider: HostingPorts["createGitHubProvider"];
  registerGitConnection: ReturnType<
    typeof import("./git-deployments").createHostingGitDeployments
  >["registerGitConnection"];
  env: HostingPorts["env"];
};

export function createHostingGitOauth(dependencies: Dependencies) {
  function gitOAuthSettings(
    env: Record<string, string | undefined> = dependencies.env,
  ) {
    try {
      const origin = new URL(env.SPITZLI_GIT_STUDIO_ORIGIN!);
      const local =
        env.NODE_ENV !== "production" &&
        origin.protocol === "http:" &&
        ["localhost", "127.0.0.1"].includes(origin.hostname);
      if (
        (!local && origin.protocol !== "https:") ||
        origin.username ||
        origin.password ||
        origin.pathname !== "/" ||
        origin.search ||
        origin.hash ||
        !env.SPITZLI_GITHUB_CLIENT_ID ||
        !/^[a-z0-9-]{1,100}$/.test(env.SPITZLI_GITHUB_APP_SLUG ?? "")
      )
        throw Error();
      return {
        clientID: env.SPITZLI_GITHUB_CLIENT_ID,
        appSlug: env.SPITZLI_GITHUB_APP_SLUG!,
        redirectURI: `${origin.origin}/api/hosting/git/callback`,
      };
    } catch {
      throw new HostingError(503, "GitHub connection setup is incomplete.");
    }
  }
  const digest = (state: string) =>
    createHash("sha256").update(state).digest("hex");
  async function executeGitOAuth(
    actor: HostingActor,
    raw: GitOAuthCommand,
    runtime?: ReturnType<Dependencies["createGitHubProvider"]>,
  ) {
    return dependencies.withHostingDatabase(actor, async () => {
      if (actor.source !== "studio")
        throw new HostingError(403, "Connect GitHub interactively in Studio.");
      const command = gitOAuthCommandSchema.parse(raw);
      const config = gitOAuthSettings();
      return dependencies.transaction(async (db) => {
        if (command.action === "git.oauth.begin") {
          await dependencies.authorizeHosting(
            actor,
            {
              permission: "org:projects:manage",
              customerID: command.customerID,
              write: true,
              tenantAdmin: true,
            },
            db,
          );
          if (command.setupID) {
            const setup = (
              await db.query(
                "SELECT project_id FROM webdock_auth.hosting_setup WHERE id=$1 AND customer_id=$2",
                [command.setupID, command.customerID],
              )
            ).rows[0];
            if (!setup)
              throw new HostingError(404, "Project setup is unavailable.");
            await dependencies.authorizeHosting(
              actor,
              {
                permission: "org:projects:manage",
                projectID: setup.project_id,
                write: true,
              },
              db,
            );
          }
          const recent = (
            await db.query(
              "SELECT count(*)::int AS count FROM webdock_auth.git_flow WHERE subject=$1 AND expires_at>now()",
              [actor.subject],
            )
          ).rows[0].count;
          if (recent >= 5)
            throw new HostingError(429, "Wait before connecting GitHub again.");
          const state = randomBytes(32).toString("base64url");
          await db.query(
            "INSERT INTO webdock_auth.git_flow(state_hash,subject,session_id,customer_id,expires_at,setup_id) VALUES($1,$2,$3,$4,now()+interval '10 minutes',$5)",
            [
              digest(state),
              actor.subject,
              actor.sessionID,
              command.customerID,
              command.setupID ?? null,
            ],
          );
          const url = new URL("https://github.com/login/oauth/authorize");
          url.search = new URLSearchParams({
            client_id: config.clientID,
            redirect_uri: config.redirectURI,
            state,
          }).toString();
          return { url: url.href, state };
        }
        const hash = digest(command.state);
        const flow = (
          await db.query(
            "SELECT * FROM webdock_auth.git_flow WHERE state_hash=$1 AND subject=$2 AND session_id=$3 AND consumed_at IS NULL AND expires_at>now() FOR UPDATE",
            [hash, actor.subject, actor.sessionID],
          )
        ).rows[0];
        if (!flow)
          throw new HostingError(
            403,
            "GitHub connection expired. Start again.",
          );
        await dependencies.authorizeHosting(
          actor,
          {
            permission: "org:projects:manage",
            customerID: flow.customer_id,
            write: true,
            tenantAdmin: true,
          },
          db,
        );
        const setup = flow.setup_id
          ? (
              await db.query(
                "SELECT id,project_id FROM webdock_auth.hosting_setup WHERE id=$1 AND customer_id=$2",
                [flow.setup_id, flow.customer_id],
              )
            ).rows[0]
          : null;
        const returnPath = setup
          ? `/hosting/projects/${setup.project_id}/setup/${setup.id}`
          : undefined;
        if (setup)
          await dependencies.authorizeHosting(
            actor,
            {
              permission: "org:projects:manage",
              projectID: setup.project_id,
              write: true,
            },
            db,
          );
        if (command.action === "git.oauth.return")
          return { customerID: flow.customer_id, returnPath };
        const provider = runtime ?? dependencies.createGitHubProvider();
        if (command.action === "git.oauth.exchange") {
          if (flow.user_token_encrypted)
            throw new HostingError(
              409,
              "GitHub authorization was already used.",
            );
          const token = await provider.exchangeOAuthCode(
            command.code,
            config.redirectURI,
          );
          await db.query(
            "UPDATE webdock_auth.git_flow SET user_token_encrypted=$2 WHERE state_hash=$1",
            [hash, dependencies.encryptMailSecret(token, `git-oauth:${hash}`)],
          );
          return { customerID: flow.customer_id };
        }
        if (!flow.user_token_encrypted)
          throw new HostingError(403, "Complete GitHub authorization first.");
        const token = dependencies.decryptMailSecret<string>(
          flow.user_token_encrypted,
          `git-oauth:${hash}`,
        );
        if (command.action === "git.oauth.options") {
          const installations = await provider.listUserInstallations(token);
          if (
            command.installationID &&
            !installations.some(
              (i) => i.installationID === command.installationID,
            )
          )
            throw new HostingError(403, "GitHub installation is unavailable.");
          return {
            customerID: flow.customer_id,
            installations,
            repositories: command.installationID
              ? await provider.listUserRepositories(
                  token,
                  command.installationID,
                )
              : [],
            installURL: `https://github.com/apps/${config.appSlug}/installations/new`,
          };
        }
        const binding = await provider.verifyRepository(
          token,
          command.installationID,
          command.repositoryID,
        );
        const connection = await dependencies.registerGitConnection(
          actor,
          binding,
          flow.customer_id,
          db,
        );
        await db.query(
          "UPDATE webdock_auth.git_flow SET consumed_at=now(),user_token_encrypted=NULL WHERE state_hash=$1",
          [hash],
        );
        if (setup)
          await db.query(
            `UPDATE webdock_auth.hosting_setup SET configuration=jsonb_set(configuration,'{source}',configuration->'source' || $2::jsonb),revision=revision+1,state='draft',preview=NULL,updated_at=now() WHERE id=$1 AND state IN ('draft','awaiting-confirmation') AND resources='{}'::jsonb AND configuration->'source'->>'kind'='git'`,
            [
              setup.id,
              JSON.stringify({
                connectionID: connection.id,
                repositoryID: binding.repositoryID,
                branch: binding.defaultBranch,
              }),
            ],
          );
        return { customerID: flow.customer_id, connection, returnPath };
      });
    });
  }
  return { gitOAuthSettings, executeGitOAuth };
}
