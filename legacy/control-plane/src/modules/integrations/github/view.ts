import {
  getGitHub,
  githubOrigin,
  type GitHubInstallation,
  type GitHubRepository,
} from "./client";
import { browserHeaders } from "../../portal/request-context";
import { currentRegistryActor } from "../operator";
export async function githubView(input: Record<string, string>) {
  const actor = await currentRegistryActor(),
    github = getGitHub();
  const positiveID = (raw: string | undefined) =>
    raw &&
    /^\d+$/.test(raw) &&
    Number.isSafeInteger(Number(raw)) &&
    Number(raw) > 0
      ? Number(raw)
      : 0;
  const pageNumber = (raw: string | undefined) => {
    const value = positiveID(raw);
    return value > 0 && value <= 10000 ? value : 1;
  };
  const installationPage = pageNumber(input.installationPage),
    page = pageNumber(input.page);
  let installation = positiveID(input.installation),
    installations: GitHubInstallation[] = [],
    repositories: GitHubRepository[] = [],
    installationPages = 1,
    repositoryPages = 1,
    problem = false;
  const session = github
    ? await github.session(browserHeaders(), actor.user.id)
    : null;
  if (github && session)
    try {
      const available = await github.installations(session, installationPage);
      installations = available.items;
      installationPages = available.totalPages;
      if (!installation) installation = installations[0]?.id || 0;
      if (installation) {
        const result = await github.repositories(
          session,
          installation,
          installationPage,
          page,
        );
        repositories = result.items;
        repositoryPages = result.totalPages;
      }
    } catch {
      problem = true;
    }
  return {
    configured: !!github,
    session: session ? { login: session.login } : null,
    installURL: github?.installURL || null,
    permissionsURL: github?.permissionsURL || null,
    callbackURL:
      github?.callbackURL ||
      new URL("/api/github/callback", githubOrigin()).href,
    installations,
    repositories,
    installationPages,
    repositoryPages,
    installation,
    installationPage,
    page,
    problem,
  };
}
