import { vercelView, linkVercelProject } from "../integrations/vercel/view";
import { projectVercelStatus } from "../integrations/vercel/vercel-project";
import { githubView } from "../integrations/github/view";
import { hostingCall } from "./hosting";
import { requireAccessOperator } from "../access/access-management";
import { currentRegistryActor } from "../integrations/operator";
import { HostingError } from "@webdock/hosting-contracts";
import type { PublicOperationError } from "./product-operation";
export async function dispatch(
  operation: string,
  args: unknown[],
  headers: Headers,
) {
  if (operation === "hosting.command" || operation === "database.command")
    return hostingCall(
      args[0],
      operation === "hosting.command" ? "hosting" : "database",
    );
  await requireAccessOperator(headers);
  const text = args[0] as string,
    input = args[0] as Record<string, string>;
  switch (operation) {
    case "integrations.vercel":
      return vercelView(text);
    case "integrations.linkVercel":
      return linkVercelProject(input);
    case "integrations.projectStatus":
      return projectVercelStatus(await currentRegistryActor(), text);
    case "integrations.github":
      return githubView(input);
  }
  throw Error("Unknown hosting operation");
}
export function publicError(error: unknown): PublicOperationError | undefined {
  if (error instanceof HostingError)
    return {
      kind: "HostingError",
      message: error.message,
      status: error.status,
    };
}
