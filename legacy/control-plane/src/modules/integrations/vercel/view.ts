import { currentRegistryActor } from "../operator";
import { vercelSettings } from "../vercel-settings";
import { loadVercelConnection, saveVercelLink } from "./vercel-store";
import { listVercelProjects, fetchVercelProject } from "./vercel-api";
import { recordID } from "../../registry/contracts";
export async function vercelView(cursor: string) {
  const actor = await currentRegistryActor(),
    settings = vercelSettings();
  let connected = false,
    problem = false,
    available: {
      projects: { id: string; name: string }[];
      nextCursor: string | null;
    } = { projects: [], nextCursor: null };
  if (settings)
    try {
      const credential = await loadVercelConnection(
        actor,
        settings.teamID,
        settings.cookieSecret,
      );
      connected = !!credential;
      if (credential)
        available = await listVercelProjects({
          token: credential.accessToken,
          teamID: settings.teamID,
          cursor: cursor || undefined,
        });
    } catch {
      problem = true;
    }
  return { configured: !!settings, connected, problem, available };
}
export async function linkVercelProject(input: Record<string, string>) {
  const actor = await currentRegistryActor(),
    settings = vercelSettings();
  if (!settings) throw Error("Vercel setup required");
  const credential = await loadVercelConnection(
    actor,
    settings.teamID,
    settings.cookieSecret,
  );
  if (!credential) throw Error("Vercel connection required");
  const project = recordID.parse(input.project),
    snapshot = await fetchVercelProject({
      token: credential.accessToken,
      teamID: settings.teamID,
      projectID: input.vercelProject,
    });
  await saveVercelLink(actor, project, settings.teamID, snapshot.projectID);
  return {
    message:
      "Vercel project linked. Open the Studio project to see live deployment data.",
  };
}
