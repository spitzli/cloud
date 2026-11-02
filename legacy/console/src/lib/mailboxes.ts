import "server-only";
import { studioCall } from "./studio-client";
import type { NativeMailboxesView, ChangeResult } from "./studio-contracts";
export function getTenantMailboxes(_headers: Headers, customerID: string): Promise<NativeMailboxesView | null> {
  return studioCall("getTenantMailboxes", [customerID]);
}
export function manageTenantMailboxes(_headers: Headers, customerID: string, input: Record<string, string>): Promise<ChangeResult> {
  return studioCall("manageTenantMailboxes", [customerID, input]);
}
