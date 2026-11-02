import "server-only";
import { studioCall } from "./studio-client";
export { TrackingError } from "./studio-errors";
import type { ChangeResult, TrackingView } from "./studio-contracts";
export function getTenantTracking(...args: [headers: Headers, customerID: string]): Promise<TrackingView> { return studioCall("getTenantTracking", args.slice(1)); }
export function manageTenantTracking(...args: [headers: Headers, customerID: string, input: Record<string, string>]): Promise<ChangeResult> { return studioCall("manageTenantTracking", args.slice(1)); }
