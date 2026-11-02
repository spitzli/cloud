"use server";
import { headers } from "next/headers";
import { revalidatePath } from "next/cache";
import { AccessError } from "@/lib/access-management";
import { managePlans, PlanError } from "@/lib/plans";
import { manageTenantStorage, StorageUsageError } from "@/lib/storage-usage";
import type { PlanState } from "@/app/admin/plans/actions";
export async function usageAction(_state: PlanState, form: FormData): Promise<PlanState> {
  try {
    const input = Object.fromEntries([...form.entries()].filter((entry): entry is [string, string] => typeof entry[1] === "string"));
    const requestHeaders = await headers();
    const result = input.action.startsWith("storage-")
      ? await manageTenantStorage(requestHeaders, input.customerID, { ...input, action: input.action.slice(8) })
      : await managePlans(requestHeaders, input);
    revalidatePath(`/tenants/${input.customerID}/usage`);
    return result;
  } catch (error) {
    // A failed inventory refresh persists a stale/error marker; refresh that view too.
    const customerID = form.get("customerID");
    if (typeof customerID === "string" && /^[1-9][0-9]{0,18}$/.test(customerID)) revalidatePath(`/tenants/${customerID}/usage`);
    return { error: error instanceof AccessError || error instanceof PlanError || error instanceof StorageUsageError ? error.message : "Could not save this change. Please try again." };
  }
}
