"use server";
import { headers } from "next/headers";
import { revalidatePath } from "next/cache";
import { AccessError } from "@/lib/access-management";
import { acceptOffer, PlanError } from "@/lib/plans";
export type OfferState = { error?: string; message?: string; customerID?: string };
export async function acceptOfferAction(token: string, _state: OfferState, form: FormData): Promise<OfferState> {
  if (form.get("confirm") !== "yes") return { error: "Confirm that you want to accept this offer and assign the plan." };
  try {
    const result = await acceptOffer(await headers(), token);
    revalidatePath(`/tenants/${result.customerID}/usage`);
    revalidatePath(`/offers/${token}`);
    return result;
  } catch (error) { return { error: error instanceof AccessError || error instanceof PlanError ? error.message : "Could not accept this offer. Please try again." }; }
}
