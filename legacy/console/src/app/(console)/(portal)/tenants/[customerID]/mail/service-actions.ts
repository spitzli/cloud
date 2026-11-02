"use server";
import { headers } from "next/headers";
import { revalidatePath } from "next/cache";
import { manageTenantMailService } from "@/lib/mail-service";
import { StudioError } from "@/lib/studio-errors";

export async function mailServiceAction(_state: { error?: string; message?: string }, form: FormData): Promise<{ error?: string; message?: string }> {
  const value = (name: string) => typeof form.get(name) === "string" ? String(form.get(name)) : "";
  try {
    const result = await manageTenantMailService(await headers(), value("customer"), { action: value("action"), revision: value("revision") });
    revalidatePath(`/tenants/${value("customer")}/mail`);
    return result;
  } catch (error) {
    return { error: error instanceof StudioError ? error.message : "Mail settings could not be changed. Refresh before trying again." };
  }
}
