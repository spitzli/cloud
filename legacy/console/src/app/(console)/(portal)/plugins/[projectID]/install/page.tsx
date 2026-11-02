import Link from "next/link";
import type { GitHubView } from "@webdock/api-client/integrations";
import type { PluginSourceView } from "@webdock/api-client/plugin-sources";
import { requireRegistryOperator } from "@/lib/registry-page";
import { studioCall } from "@/lib/studio-client";
import { PluginInstaller } from "./installer";

export default async function InstallPlugin({
  params,
  searchParams,
}: {
  params: Promise<{ projectID: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  await requireRegistryOperator();
  const { projectID } = await params;
  const query = await searchParams;
  const input = Object.fromEntries(
    Object.entries(query).filter(
      (entry): entry is [string, string] => typeof entry[1] === "string",
    ),
  );
  const [sources, github] = await Promise.all([
    studioCall<PluginSourceView>("plugins.sources", [
      { action: "list", projectID },
    ]),
    studioCall<GitHubView>("plugins.github", [input]),
  ]);
  return (
    <div>
      <Link href={`/plugins/${encodeURIComponent(projectID)}`}>
        Back to project extensions
      </Link>
      <div className="page-heading">
        <div>
          <h1>Install a plugin</h1>
          <p>
            Import a source and review its built image and permissions. The
            approved plugin is then deployed and activated automatically.
          </p>
        </div>
      </div>
      <PluginInstaller
        key={`${projectID}:${github.installation}:${github.installationPage}:${github.page}`}
        projectID={projectID}
        initialSources={sources}
        github={github}
        githubNotice={
          typeof query.github === "string" ? query.github : undefined
        }
      />
    </div>
  );
}
