
import { getRequestI18n } from '@webdock/i18n/next';
import Link from "next/link";
import type { GitHubRepository } from "@webdock/api-client/integrations";

export async function GitHubRepositories({
  repositories,
  projects,
  installation,
  installationPage,
  page,
  totalPages,
}: {
  repositories: GitHubRepository[];
  projects: { id: string; name: string; repositoryURL?: string | null }[];
  installation: number;
  installationPage: number;
  page: number;
  totalPages: number;
}) {
  const i18n = await getRequestI18n();

  const pageURL = (next: number) =>
    `/integrations?${new URLSearchParams({ installation: String(installation), installationPage: String(installationPage), page: String(next) })}`;
  return (
    <>
      {repositories.length ? (
        <form className="editor" action="/api/github/select" method="post">
          <input type="hidden" name="installation" value={installation} />
          <input
            type="hidden"
            name="installationPage"
            value={installationPage}
          />
          <input type="hidden" name="page" value={page} />
          <div className="field wide">
            <label htmlFor="github-repository">{i18n.t("Repository ")}<span aria-hidden="true">*</span>
            </label>
            <select
              id="github-repository"
              name="repository"
              required
              defaultValue=""
            >
              <option value="" disabled>{i18n.t("Choose a repository")}</option>
              {repositories.map((repo) => (
                <option key={repo.id} value={repo.id}>
                  {repo.name} · {repo.private ? i18n.t("Private") : i18n.t("Public")}
                </option>
              ))}
            </select>
          </div>
          <div className="field wide">
            <label htmlFor="github-project">{i18n.t("Webdock project ")}<span aria-hidden="true">*</span>
            </label>
            <select
              id="github-project"
              name="project"
              required
              defaultValue=""
              disabled={!projects.length}
              aria-describedby="github-project-hint"
            >
              <option value="" disabled>{i18n.t("Choose a project")}</option>
              {projects.map((project) => (
                <option key={project.id} value={project.id}>
                  {project.name}
                  {project.repositoryURL ? i18n.t(" · Replace current repository") : ""}
                </option>
              ))}
            </select>
            <small id="github-project-hint">{i18n.t("Saves the repository link to the project and records the change in Activity.")}</small>
          </div>
          <div className="form-footer">
            <button
              className="button"
              type="submit"
              disabled={!projects.length}
            >{i18n.t("Link repository")}</button>
            {!projects.length && (
              <Link href="/projects/new">{i18n.t("Create a project first")}</Link>
            )}
          </div>
        </form>
      ) : (
        <p>{i18n.t("No repositories are available on this page. Review the app’s selected repositories on GitHub.")}</p>
      )}
      {totalPages > 1 && (
        <nav className="pagination" aria-label={i18n.t("Repository pages")}>
          {page > 1 && (
            <Link className="button secondary" href={pageURL(page - 1)}>{i18n.t("Previous repositories")}</Link>
          )}
          <span>{i18n.t("Page ")}{page}{i18n.t(" of ")}{totalPages}
          </span>
          {page < totalPages && (
            <Link className="button secondary" href={pageURL(page + 1)}>{i18n.t("Next repositories")}</Link>
          )}
        </nav>
      )}
    </>
  );
}
