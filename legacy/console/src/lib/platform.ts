import "server-only";
import { studioCall } from "./studio-client";
export { PlatformError } from "./studio-errors";
import type { PlatformSettings } from "./studio-contracts";
export function getPlatformSettings(...args: []): Promise<PlatformSettings> { return studioCall("getPlatformSettings", args); }
export function getMailConnectionStatus(...args: []): Promise<{ configured: boolean; consumerKeySuffix?: string }> { return studioCall("getMailConnectionStatus", args); }
export function savePlatformSettings(...args: [actorID: string, input: Record<string, string>]): Promise<void> { return studioCall("savePlatformSettings", args.slice(1)); }
export function saveMailCredentials(...args: [actorID: string, consumerKey: string, consumerSecret: string]): Promise<void> { return studioCall("saveMailCredentials", args.slice(1)); }
export function clearMailCredentials(...args: [actorID: string]): Promise<void> { return studioCall("clearMailCredentials", args.slice(1)); }
export function testMailConnection(...args: []): Promise<{ count: number }> { return studioCall("testMailConnection", args); }
export type { PlatformSettings } from "./studio-contracts";
