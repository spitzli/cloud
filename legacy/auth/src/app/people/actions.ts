"use server";
import { headers } from "next/headers";
import { revalidatePath } from "next/cache";
import { AccessError, manageAccess } from "@/lib/access-management";
export type AccessState = { error?: string; message?: string };
export async function accessAction(
  _state: AccessState,
  form: FormData,
): Promise<AccessState> {
  try {
    const input = Object.fromEntries(
      [...form].filter(
        (entry): entry is [string, string] => typeof entry[1] === "string",
      ),
    );
    const result = await manageAccess(await headers(), input);
    revalidatePath("/people");
    revalidatePath("/sites");
    return { message: result.message };
  } catch (error) {
    revalidatePath("/people");
    return {
      error:
        error instanceof AccessError
          ? error.message
          : "The operation could not be completed. Some changes may have been saved. Refresh and review access or invitation status before retrying.",
    };
  }
}
