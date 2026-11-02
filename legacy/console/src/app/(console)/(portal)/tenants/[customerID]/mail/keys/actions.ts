"use server";
import { headers } from "next/headers";
import { revalidatePath } from "next/cache";
import { AccessError } from "@/lib/access-management";
import { MailKeysError, manageTenantMailKeys } from "@/lib/mail-keys";
export type KeyState = {
  error?: string;
  message?: string;
  created?: { consumerKey: string; consumerSecret: string };
};
export async function keyAction(
  _previous: KeyState,
  form: FormData,
): Promise<KeyState> {
  const value = (name: string) =>
    typeof form.get(name) === "string" ? String(form.get(name)) : "";
  try {
    return await manageTenantMailKeys(await headers(), value("customer"), {
      action: value("action"),
      label: value("label"),
      permissions: form.getAll("permissions").map(String),
      ips: value("ips")
        .split(/[\s,]+/)
        .filter(Boolean),
      consumerKey: value("consumerKey"),
      confirm: value("confirm"),
    });
  } catch (error) {
    return {
      error:
        error instanceof MailKeysError || error instanceof AccessError
          ? error.message
          : "Could not complete this credential change. Refresh before retrying.",
    };
  } finally {
    revalidatePath("/tenants", "layout");
  }
}
