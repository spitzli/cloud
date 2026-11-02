import Link from "next/link";
import { headers } from "next/headers";
import { getRequestI18n } from "@webdock/i18n/next";
import { hostingCall } from "@/lib/hosting-client";
import { readGitFlowCookie } from "@webdock/api-client/flow-cookies";

export default async function GitConnection({
  searchParams,
}: {
  searchParams: Promise<{ installation?: string }>;
}) {
  const { t } = await getRequestI18n();
  const query = await searchParams;
  let data: {
    installations: { installationID: string; accountLogin: string }[];
    repositories: { repositoryID: string; fullName: string }[];
    installURL: string;
  };
  try {
    const state = readGitFlowCookie(
      await headers(),
      (process.env.WEBDOCK_STUDIO_ORIGIN || process.env.WEBDOCK_STUDIO_ORIGIN || process.env.NEXT_PUBLIC_SERVER_URL || "https://studio.webdock.dev"),
    );
    data = await hostingCall({
      action: "git.oauth.options",
      state,
      ...(query.installation ? { installationID: query.installation } : {}),
    });
  } catch {
    return (
      <section className="panel">
        <h1>{t("Connect GitHub")}</h1>
        <p role="alert">{t("GitHub connection expired. Start again.")}</p>
      </section>
    );
  }
  return (
    <>
      <h1>{t("Connect GitHub")}</h1>
      <section className="panel">
        <p>
          {t(
            "Choose an installation and repository you can access. GitHub consent is completed on GitHub.",
          )}
        </p>
        <a
          href={data.installURL}
          target="_blank"
          rel="noopener noreferrer"
          className="button secondary"
        >
          {t("Install or update GitHub access")}
        </a>
        <form method="get">
          <label className="field">
            {t("GitHub account")}
            <select
              name="installation"
              required
              defaultValue={query.installation ?? ""}
            >
              <option value="" disabled>
                {t("Select an installation")}
              </option>
              {data.installations.map((i) => (
                <option key={i.installationID} value={i.installationID}>
                  {i.accountLogin}
                </option>
              ))}
            </select>
          </label>
          <button className="button">{t("Load repositories")}</button>
        </form>
        {query.installation && (
          <form action="/api/hosting/git/select" method="post">
            <input
              type="hidden"
              name="installationID"
              value={query.installation}
            />
            <label className="field">
              {t("Repository")}
              <select name="repositoryID" required defaultValue="">
                <option value="" disabled>
                  {t("Select a repository")}
                </option>
                {data.repositories.map((r) => (
                  <option value={r.repositoryID} key={r.repositoryID}>
                    {r.fullName}
                  </option>
                ))}
              </select>
            </label>
            <button className="button" disabled={!data.repositories.length}>
              {t("Connect repository")}
            </button>
          </form>
        )}
        <Link href="/hosting/git/connection-error">{t("Connection help")}</Link>
      </section>
    </>
  );
}
