import "server-only";
import { studioCall } from "./studio-client";
export { MailKeysError } from "./studio-errors";
import type { ChangeResult, MailKeyInput, MailKeysView } from "./studio-contracts";
export function getTenantMailKeys(...args: [headers: Headers, customerID: string]): Promise<MailKeysView> { return studioCall("getTenantMailKeys", args.slice(1)); }
export function manageTenantMailKeys(...args: [headers: Headers, customerID: string, input: MailKeyInput]): Promise<ChangeResult & { created?: { consumerKey: string; consumerSecret: string } }> { return studioCall("manageTenantMailKeys", args.slice(1)); }
export type { MailKeyInput } from "./studio-contracts";
