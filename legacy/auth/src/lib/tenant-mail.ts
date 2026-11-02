import "server-only";
import { studioCall } from "./studio-client";
export { TenantMailError } from "./studio-errors";
import type { ChangeResult, MailView } from "./studio-contracts";
export function getTenantMail(...args: [headers: Headers, customerID: string]): Promise<MailView> { return studioCall("getTenantMail", args.slice(1)); }
export function manageTenantMail(...args: [headers: Headers, customerID: string, input: Record<string, string>]): Promise<ChangeResult> { return studioCall("manageTenantMail", args.slice(1)); }
export type { MailView } from "./studio-contracts";
