"use server";
import { studioCall } from "./studio-client";
import { StudioError } from "./studio-errors";
export type WebmailLaunch = { code: string; gatewayOrigin: string; customerID: string; mailboxID: string; expiresAt: string };
export async function launchWebmail(customerID: string, mailboxID: string): Promise<{ launch?: WebmailLaunch; error?: string }> {
  if (![customerID, mailboxID].every(value=>/^[1-9][0-9]{0,18}$/.test(value))) return { error: "Mail access is unavailable." };
  try { return { launch: await studioCall<WebmailLaunch>("launchTenantWebmail", [customerID, mailboxID]) }; }
  catch (error) { return { error: error instanceof StudioError ? error.message : "Mail access is unavailable." }; }
}
