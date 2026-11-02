"use server";

import { headers } from "next/headers";
import { revalidatePath } from "next/cache";
import { AccessError } from "@/lib/access-management";
import { PlatformError } from "@/lib/platform";
import { TenantMailError, manageTenantMail } from "@/lib/tenant-mail";
import { MailDomainsError, manageTenantSenderDomain } from "@/lib/mail-domains";

export async function mailAction(_state: { error?: string; message?: string }, form: FormData): Promise<{ error?: string; message?: string }> {
  const value = (name: string) => typeof form.get(name) === "string" ? String(form.get(name)) : "";
  const input = Object.fromEntries(["action", "email", "firstName", "lastName", "policyAgree", "providerID", "confirm", "active", "limit", "domain", "domainID"].map(name => [name, value(name)]));
  try {
    if (["register-sender", "refresh-sender", "unregister-sender"].includes(input.action)) {
      const action = input.action === "register-sender" ? "register" : input.action === "refresh-sender" ? "refresh" : "unregister";
      return await manageTenantSenderDomain(await headers(), value("customer"), { action, domainID: input.domainID, confirm: input.confirm });
    }
    return await manageTenantMail(await headers(), value("customer"), input);
  } catch (error) {
    return { error: error instanceof AccessError || error instanceof PlatformError || error instanceof TenantMailError || error instanceof MailDomainsError ? error.message : "Could not complete this Mail change. Refresh and review the current state before taking another action." };
  } finally {
    revalidatePath("/tenants", "layout");
  }
}
