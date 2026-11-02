import "server-only";
import { studioCall } from "./studio-client";
import type { NativeMailView, ChangeResult } from "./studio-contracts";
export { MailServiceError } from "./studio-errors";
export function getTenantMailService(_headers: Headers, customerID: string): Promise<NativeMailView> {
  return studioCall("getTenantMailService", [customerID]);
}
export function manageTenantMailService(_headers: Headers, customerID: string, input: Record<string, string>): Promise<ChangeResult> {
  return studioCall("manageTenantMailService", [customerID, input]);
}
