import "server-only";
import { studioCall } from "./studio-client";
export { AccessError } from "./studio-errors";
import type { ChangeResult, Site, AccessView } from "./studio-contracts";
import type { StudioSession } from "./studio-client";
export function requireAccessOperator(...args: [headers: Headers]): Promise<StudioSession> { return studioCall("requireAccessOperator", args.slice(1)); }
export function listAccess(...args: [headers: Headers, search: string]): Promise<AccessView> { return studioCall("listAccess", args.slice(1)); }
export function accountSites(...args: [headers: Headers]): Promise<Site[]> { return studioCall("accountSites", args.slice(1)); }
export function manageAccess(...args: [headers: Headers, input: Record<string, string>]): Promise<ChangeResult & { id?: string }> { return studioCall("manageAccess", args.slice(1)); }
