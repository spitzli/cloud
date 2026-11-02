"use server";
import { headers } from "next/headers";
import { revalidatePath } from "next/cache";
import { AccessError } from "@/lib/access-management";
import { TrackingError, manageTenantTracking } from "@/lib/mail-tracking";
export async function trackingAction(_state: { error?: string; message?: string }, form: FormData): Promise<{ error?: string; message?: string }> {
 const value = (name: string) => typeof form.get(name) === "string" ? String(form.get(name)) : "";
 try { return await manageTenantTracking(await headers(), value("customer"), Object.fromEntries(["action", "senderDomainID", "prefix", "domainID", "setting", "value", "confirm"].map(name => [name, value(name)]))); }
 catch (error) { return { error: error instanceof TrackingError || error instanceof AccessError ? error.message : "Could not complete tracking setup. Refresh to check the current state." }; }
 finally { revalidatePath("/tenants", "layout"); }
}
