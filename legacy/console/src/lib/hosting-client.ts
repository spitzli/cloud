import {studioCall} from "./studio-client";
import {AccessError,StudioError} from "./studio-errors";
import { withHostingPageAccess, HostingSessionRequiredError } from "./hosting-page-access";
import "server-only";
import { HostingError, type HostingCommand } from "@webdock/hosting-contracts";
import type { GitOAuthCommand } from "@webdock/hosting-contracts/git-oauth";
export async function hostingCall<T = unknown>(
  command:
    HostingCommand | GitOAuthCommand | {action:"databases.credentials.display";databaseID:string} | { action: "enrollment.display"; enrollmentID: string },
): Promise<T> {
  try{return await studioCall<T>("hosting.command",[command]);}
  catch(error){if(error instanceof AccessError&&error.status===401)throw new HostingSessionRequiredError();if(error instanceof StudioError)throw new HostingError(error.status,error.message);throw error;}
}

/** Server Component entrypoint. Mutations continue using hostingCall to preserve drafts. */
export function hostingPageCall<T = unknown>(
  command: HostingCommand,
  returnTo: string,
): Promise<T> {
  return withHostingPageAccess(returnTo, () => hostingCall<T>(command));
}
