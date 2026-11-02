"use server";
import { headers } from "next/headers";
import { revalidatePath } from "next/cache";
import { AccessError } from "@/lib/access-management";
import { PlanError, managePlans } from "@/lib/plans";
export type PlanState = { error?: string; message?: string; offerURL?: string };
export async function planAction(_state: PlanState, form: FormData): Promise<PlanState> {
  try {
    const input = Object.fromEntries([...form.entries()].filter((entry): entry is [string, string] => typeof entry[1] === "string"));
    const result = await managePlans(await headers(), input);
    revalidatePath("/admin/plans");
    if (/^[1-9][0-9]{0,18}$/.test(input.customerID || "")) revalidatePath(`/tenants/${input.customerID}/usage`);
    return result;
  } catch (error) { return { error: error instanceof PlanError || error instanceof AccessError ? error.message : "Could not save this plan change. Please try again." }; }
}
