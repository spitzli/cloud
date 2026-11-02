import { RegistryError as APIError } from "../../registry/service";
import { z } from "zod";
import { recordID } from "../../registry/contracts";
import { registryService } from "../../registry/context";
import type { RegistryActor } from "../operator";
import type { configureGitHub, GitHubSession } from "./client";

const positive = z.coerce
  .number()
  .int()
  .positive()
  .max(Number.MAX_SAFE_INTEGER);
export const githubSelection = z
  .object({
    project: recordID,
    installation: positive,
    installationPage: positive.max(10000),
    page: positive.max(10000),
    repository: positive,
  })
  .strict();

export async function linkGitHubRepository(
  actor: Pick<RegistryActor, "identity" | "user">,
  github: ReturnType<typeof configureGitHub>,
  session: GitHubSession,
  input: unknown,
) {
  if (
    actor.user.collection !== "users" ||
    actor.user.role !== "operator" ||
    session.operatorID !== actor.user.id
  )
    throw new APIError("An operator is required.", 403);
  const { project: id, ...selection } = githubSelection.parse(input);
  // Re-read GitHub membership at save time; never trust a URL or repository from the browser.
  const repository = await github.repository(session, selection);
  const registry = await registryService();
  const project = await registry.getRecord(actor.identity, "projects", id);
  if (!("status" in project) || project.status !== "active")
    throw new APIError(
      "Restore this project before linking a repository.",
      409,
    );
  return registry.patchRepository(actor.identity, id, repository.url);
}
