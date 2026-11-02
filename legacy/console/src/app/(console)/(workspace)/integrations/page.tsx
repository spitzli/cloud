import {notFound} from "next/navigation";
import {getStudioSession} from "@/lib/studio-client";
import {studioCall} from "@/lib/studio-client";
import type {GitHubView} from "@webdock/api-client/integrations";

import { msgid } from '@webdock/i18n';

import { getRequestI18n } from '@webdock/i18n/next';
import { Suspense } from "react";
import { VercelIntegration } from "../../../../components/vercel-integration";
import Link from "next/link";
import { requireRegistryOperator } from "../../../../lib/registry-page";
import { GitHubRepositories } from "../../../../components/github-repositories";
const mcpResource=()=>new URL("/api/mcp",process.env.WEBDOCK_STUDIO_ORIGIN||process.env.NEXT_PUBLIC_SERVER_URL||"https://studio.webdock.dev").href;

const messages: Record<string, string> = {
  connected: msgid("GitHub connected. Choose an installation and a repository below."),
  disconnected:
    msgid("GitHub disconnected from this browser. Existing project links are unchanged."),
  failed: msgid("GitHub authorization did not complete. Start Connect GitHub again."),
  expired:
    msgid("Your GitHub connection expired. Connect again to choose repositories."),
  "save-failed":
    msgid("The repository could not be linked. Refresh the list and check that the project and GitHub installation are still accessible."),
  setup: msgid("Complete the GitHub App setup before connecting."),
};

export default async function Integrations({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  if (!["enabled", "draining"].includes((await getStudioSession())?.installation?.modules.hosting ?? "")) notFound();
  const i18n = await getRequestI18n();

  const { payload, user } = await requireRegistryOperator();
  const query = await searchParams;
  const input=Object.fromEntries(Object.entries(query).filter((entry):entry is [string,string]=>typeof entry[1]==="string"));
  const view=await studioCall<GitHubView>("integrations.github",[input]);
  const {session,installationPage,page,installations,installationPages,repositories,repositoryPages,installation}=view;
  const github=view.configured?{installURL:view.installURL||"",permissionsURL:view.permissionsURL||"",callbackURL:view.callbackURL}:null;
  const problem=view.problem?msgid("GitHub access could not be checked. Reconnect or review the app’s installation and read-only metadata permission."):"";
  const projects = session
    ? await payload.find({
        collection: "projects",
        where: { status: { equals: "active" } },
        limit: 1000,
        sort: "name",
        depth: 0,
        user,
        overrideAccess: false,
      })
    : null;
  const notice =
    typeof query.github === "string" ? messages[query.github] : null;
  const isError = ["failed", "expired", "save-failed"].includes(
    String(query.github),
  );
  const callback =
    github?.callbackURL || view.callbackURL;
  return (
    <>
      <div className="page-heading">
        <div>
          <h1>{i18n.t("Connections")}</h1>
          <p>{i18n.t("Connect your repositories and the tools you work with.")}</p>
        </div>
      </div>
      {notice && (
        <p
          className={isError ? "error" : "success"}
          role={isError ? "alert" : "status"}
        >
          {i18n.t(notice)}
        </p>
      )}
      <section className="panel"><h2>{i18n.t("Connection directory")}</h2>
        <p>{i18n.t("Platform connections serve Webdock. Customer-owned connections stay in their customer workspace.")}</p>
        <ul className="workspace-list">
          <li><Link href="#platform-vercel">Vercel</Link><p className="muted">{i18n.t("Platform · Project hosting")}</p></li>
          <li><Link href="#github-heading">GitHub</Link><p className="muted">{i18n.t("Operator account · Repository selection")}</p></li>
          <li><Link href="/admin">{i18n.t("Platform Mail settings")}</Link><p className="muted">{i18n.t("Platform · Managed sending")}</p></li>
          <li><Link href="/customers">{i18n.t("Customer connections")}</Link><p className="muted">{i18n.t("Choose a customer for their infrastructure, Mail, Git deployment connections and measurement sources.")}</p></li>
        </ul>
      </section>
      <div id="platform-vercel"/>
      <Suspense
        fallback={
          <section className="panel">
            <h2>{i18n.t("Vercel")}</h2>
            <p role="status">{i18n.t("Loading connection…")}</p>
          </section>
        }
      >
        <VercelIntegration
          status={typeof query.vercel === "string" ? query.vercel : undefined}
          cursor={
            typeof query.vercelCursor === "string"
              ? query.vercelCursor
              : undefined
          }
        />
      </Suspense>
      <div className="detail-grid">
        <section className="panel" aria-labelledby="github-heading">
          <div className="section-heading">
            <h2 id="github-heading">{i18n.t("GitHub")}</h2>
            <span className={`badge ${session ? "connected" : ""}`}>
              {session
                ? i18n.t("Connected")
                : github
                  ? i18n.t("Not connected")
                  : i18n.t("Setup required")}
            </span>
          </div>
          <p>{i18n.t("Choose repositories from a GitHub App installation and link them to Webdock projects.")}</p>
          {session ? (
            <>
              <p>{i18n.t("Connected as ")}<strong>{session.login}</strong>{i18n.t(". This browser connection expires after eight hours.")}</p>
              <div className="section-heading">
                <a className="button secondary" href={github!.installURL}>{i18n.t("Manage selected repositories")}</a>
                <form action="/api/github/disconnect" method="post">
                  <button className="text-button" type="submit">{i18n.t("Disconnect this browser")}</button>
                </form>
              </div>
              {problem ? (
                <>
                  <p className="error" role="alert">
                    {i18n.error(problem)}
                  </p>
                  <form action="/api/github/connect" method="post">
                    <button className="button" type="submit">{i18n.t("Reconnect GitHub")}</button>
                  </form>
                </>
              ) : (
                <>
                  {installations.length ? (
                    <>
                      <form
                        className="editor"
                        action="/integrations"
                        method="get"
                      >
                        <input
                          type="hidden"
                          name="installationPage"
                          value={installationPage}
                        />
                        <div className="field wide">
                          <label htmlFor="github-installation">{i18n.t("GitHub account or organisation")}</label>
                          <select
                            id="github-installation"
                            name="installation"
                            defaultValue={installation}
                          >
                            {installations.map((item) => (
                              <option key={item.id} value={item.id}>
                                {item.account}
                              </option>
                            ))}
                          </select>
                        </div>
                        <div className="form-footer">
                          <button className="button secondary" type="submit">{i18n.t("Browse repositories")}</button>
                        </div>
                      </form>
                      <GitHubRepositories
                        repositories={repositories}
                        projects={projects?.docs || []}
                        installation={installation}
                        installationPage={installationPage}
                        page={page}
                        totalPages={repositoryPages}
                      />
                    </>
                  ) : (
                    <p>{i18n.t("No accessible installations found. Install the app on a GitHub account and choose the repositories to make available.")}</p>
                  )}
                  {installationPages > 1 && (
                    <nav
                      className="pagination"
                      aria-label={i18n.t("GitHub installation pages")}
                    >
                      {installationPage > 1 && (
                        <Link
                          href={`/integrations?installationPage=${installationPage - 1}`}
                        >{i18n.t("Previous accounts")}</Link>
                      )}
                      <span>{i18n.t("Accounts page ")}{installationPage}{i18n.t(" of ")}{installationPages}
                      </span>
                      {installationPage < installationPages && (
                        <Link
                          href={`/integrations?installationPage=${installationPage + 1}`}
                        >{i18n.t("Next accounts")}</Link>
                      )}
                    </nav>
                  )}
                </>
              )}
              <p>
                <a href={github!.permissionsURL}>{i18n.t("Review or revoke access on GitHub")}</a>
              </p>
            </>
          ) : github ? (
            <>
              <form action="/api/github/connect" method="post">
                <button className="button" type="submit">{i18n.t("Connect GitHub")}</button>
              </form>
              <p>
                <a href={github.installURL}>{i18n.t("Install the GitHub App")}</a>{i18n.t(" and choose only the repositories you want to use.")}</p>
            </>
          ) : (
            <p>{i18n.t("Register a GitHub App once to enable repository selection for Studio.")}</p>
          )}
          <details open={!github}>
            <summary>{i18n.t("GitHub App setup")}</summary>
            <ol>
              <li>
                <a href="https://github.com/settings/apps/new">{i18n.t("Register a GitHub App")}</a>{" "}{i18n.t("with the callback URL")}{" "}
                <code style={{ overflowWrap: "anywhere" }}>{callback}</code>.
              </li>
              <li>{i18n.t("Source import requires")}{" "}
                <strong>{i18n.t("Repository permissions → Metadata → Read-only; Contents → Read-only")}</strong>{i18n.t(". Source import only reads repositories. Actions → Read and write enables automatic builds in the private build repository; existing Checks → Write may remain. Keep user access token expiration enabled.")}</li>
              <li>{i18n.t("Set ")}<code>WEBDOCK_GITHUB_CLIENT_ID</code>,{" "}
                <code>WEBDOCK_GITHUB_CLIENT_SECRET</code>{i18n.t(", and")}{" "}
                <code>WEBDOCK_GITHUB_APP_SLUG</code>{i18n.t(" in Studio’s server environment. Studio uses its existing SSO cookie secret to encrypt the connection.")}</li>
              <li>{i18n.t("Install the app with ")}<strong>{i18n.t("Only select repositories")}</strong>{i18n.t(", then return here and choose ")}<strong>{i18n.t("Connect GitHub")}</strong>{i18n.t(". Leave automatic user authorization during installation off so this connection starts in Studio.")}</li>
            </ol>
            <p>
              <a href="https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app">{i18n.t("GitHub App authorization documentation")}</a>
            </p>
          </details>
        </section>
        <aside>
          <section className="panel cms-panel" aria-labelledby="mcp-heading">
            <h2 id="mcp-heading">{i18n.t("Webdock MCP")}</h2>
            <p>{i18n.t("Use Webdock’s project registry from an MCP-compatible assistant. Authorization uses your central Webdock account.")}</p>
            <dl>
              <dt>{i18n.t("Server URL")}</dt>
              <dd>
                <code style={{ overflowWrap: "anywhere" }}>
                  {mcpResource()}
                </code>
              </dd>
              <dt>{i18n.t("HTTP API")}</dt>
              <dd>
                <code>{new URL("/api/registry", mcpResource()).href}</code>
              </dd>
              <dt>{i18n.t("Read scope")}</dt>
              <dd>
                <code>webdock:read</code>{i18n.t(" — browse customers, projects, CMS instances, and activity.")}</dd>
              <dt>{i18n.t("Write scope")}</dt>
              <dd>
                <code>webdock:write</code>{i18n.t(" — create, update and explicitly delete managed projects. Platform administrators only.")}</dd>
            </dl>
            <a
              className="button secondary"
              href="https://auth.webdock.dev/connections"
            >{i18n.t("Manage connections")}</a>
            <p>{i18n.t("Enable automatic token renewal when registering a client for remote work. Interactive sign-in and consent still require your browser. Only operators can access the registry. Changes made through integrations appear in Activity.")}</p>
          </section>
        </aside>
      </div>
    </>
  );
}
