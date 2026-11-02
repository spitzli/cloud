"use server";

import { headers } from "next/headers";
import { revalidatePath } from "next/cache";
import { AccessError, requireAccessOperator } from "@/lib/access-management";
import {
  PlatformError,
  savePlatformSettings,
  saveMailCredentials,
  clearMailCredentials,
  testMailConnection,
} from "@/lib/platform";

export type AdminState = { error?: string; message?: string };

export async function adminAction(
  _state: AdminState,
  form: FormData,
): Promise<AdminState> {
  try {
    const session = await requireAccessOperator(await headers());
    const value = (name: string) =>
      typeof form.get(name) === "string" ? String(form.get(name)) : "";
    let message: string;
    switch (value("action")) {
      case "settings":
        await savePlatformSettings(
          session.user.id,
          Object.fromEntries(
            [
              "name",
              "supportEmail",
              "cmsEnabled",
              "mailEnabled",
              "mailGatewayEnabled",
              "mailDispatchEnabled",
              "mailWebhooksEnabled",
              "mailSendingIP",
              "mailDefaultLimit",
              "mailRegion",
            ].map((key) => [key, value(key)]),
          ),
        );
        revalidatePath("/", "layout");
        message = "Webdock settings saved.";
        break;
      case "credentials": {
        const key = value("consumerKey").trim();
        const secret = value("consumerSecret").trim();
        if (!key || !secret)
          return {
            error:
              "Enter both the consumer key and consumer secret to replace the saved credentials.",
          };
        await saveMailCredentials(session.user.id, key, secret);
        message = "Mail credentials saved.";
        break;
      }
      case "test": {
        const result = await testMailConnection();
        message = `Connection successful. ${result.count} subaccount${result.count === 1 ? "" : "s"} available.`;
        break;
      }
      case "clear":
        if (value("confirm") !== "yes")
          return {
            error:
              "Confirm that you want to remove the saved Mail credentials.",
          };
        await clearMailCredentials(session.user.id);
        message =
          "Mail credentials removed. New live Mail operations are unavailable until credentials are configured again.";
        break;
      default:
        return { error: "Unknown administration action." };
    }
    revalidatePath("/admin");
    return { message };
  } catch (error) {
    return {
      error:
        error instanceof PlatformError || error instanceof AccessError
          ? error.message
          : "Could not complete this request. Check your operator session and Mail configuration, then try again.",
    };
  }
}
