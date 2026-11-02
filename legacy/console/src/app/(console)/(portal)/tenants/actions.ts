"use server";
import { headers } from "next/headers";
import { revalidatePath } from "next/cache";
import { AccessError } from "@/lib/access-management";
import { manageTenant } from "@/lib/tenants";
export async function tenantAction(
  _state: { error?: string; message?: string },
  form: FormData,
): Promise<{ error?: string; message?: string }> {
  const input = Object.fromEntries(
    [...form].filter(
      (entry): entry is [string, string] => typeof entry[1] === "string",
    ),
  );
  try {
    const result = await manageTenant(await headers(), input.customer, input);
    revalidatePath("/tenants", "layout");
    revalidatePath("/sites");
    revalidatePath("/people");
    return result;
  } catch (error) {
    revalidatePath("/tenants", "layout");
    return {
      error:
        error instanceof AccessError
          ? error.message
          : "Could not complete this change. Refresh and review the current state before retrying.",
    };
  }
}
