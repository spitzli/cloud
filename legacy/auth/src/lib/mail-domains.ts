import "server-only";
import { studioCall } from "./studio-client";
export { MailDomainsError } from "./studio-errors";
import type { ChangeResult, TenantSenderDomain } from "./studio-contracts";
export function getTenantSenderDomains(...args: [headers: Headers, customerID: string]): Promise<{ domains: TenantSenderDomain[] }> { return studioCall("getTenantSenderDomains", args.slice(1)); }
export function manageTenantSenderDomain(...args: [headers: Headers, customerID: string, input: { action: "register" | "refresh" | "unregister"; domainID: string; confirm?: string }]): Promise<ChangeResult> { return studioCall("manageTenantSenderDomain", args.slice(1)); }
export type { TenantSenderDomain } from "./studio-contracts";
