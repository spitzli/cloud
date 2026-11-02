import "server-only";
import { studioCall } from "./studio-client";
export { StorageUsageError } from "./studio-errors";
import type { ChangeResult, StorageView } from "./studio-contracts";
export function getTenantStorage(...args: [headers: Headers, customerID: string]): Promise<StorageView> { return studioCall("getTenantStorage", args.slice(1)); }
export function manageTenantStorage(...args: [headers: Headers, customerID: string, input: Record<string, string>]): Promise<ChangeResult> { return studioCall("manageTenantStorage", args.slice(1)); }
