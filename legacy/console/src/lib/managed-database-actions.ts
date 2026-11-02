"use server";
import type {
  DatabaseOffer,
  DatabaseOrderState,
  DatabaseCredentials,
} from "@webdock/module-databases/ui-types";
export type {
  DatabaseOffer,
  DatabaseOrderState,
  DatabaseCredentials,
} from "@webdock/module-databases/ui-types";
import { revalidatePath } from "next/cache";
import {
  hostingError,
  resourceID,
  databasePlanSchema,
  type DatabasePlan,
} from "@webdock/hosting-contracts";
import { hostingCall } from "./hosting-client";

export async function prepareDatabaseOffer(
  _state: DatabaseOrderState,
  form: FormData,
): Promise<DatabaseOrderState> {
  try {
    return {
      offer: await hostingCall<DatabaseOffer>({
        action: "databases.offers.create",
        projectID: String(form.get("projectID")),
        planRevisionID: String(form.get("planRevisionID")),
        name: String(form.get("name")),
        ...(form.get("databaseID")
          ? { databaseID: String(form.get("databaseID")) }
          : {}),
        idempotencyKey: String(form.get("idempotencyKey")),
      }),
    };
  } catch (error) {
    return { error: hostingError(error).message };
  }
}
export async function acceptDatabaseOffer(
  _state: DatabaseOrderState,
  form: FormData,
): Promise<DatabaseOrderState> {
  try {
    if (form.get("confirmPrice") !== "yes")
      return { error: "Confirm the monthly price and terms." };
    const result = await hostingCall<{ databaseID: string }>({
      action: "databases.acceptOrder",
      projectID: String(form.get("projectID")),
      offerID: String(form.get("offerID")),
      confirmInterruption: form.get("confirmInterruption") === "yes",
      idempotencyKey: String(form.get("idempotencyKey")),
    });
    revalidatePath("/hosting", "layout");
    revalidatePath("/tenants", "layout");
    return { databasePath: "/hosting/databases/" + result.databaseID };
  } catch (error) {
    return { error: hostingError(error).message };
  }
}
export async function createDatabasePlan(
  _state: { error?: string; message?: string },
  form: FormData,
): Promise<{ error?: string; message?: string }> {
  try {
    const kind = String(form.get("kind"));
    const price = String(form.get("monthlyPrice")).replace(",", ".");
    if (!/^\d{1,5}(\.\d{1,2})?$/.test(price))
      return { error: "Enter a positive monthly price." };
    const shared = kind === "shared";
    const plan = databasePlanSchema.parse({
      kind,
      name: String(form.get("name")),
      monthlyPriceCents: Math.round(Number(price) * 100),
      currency: "EUR",
      postgresMajor: 18,
      cpuMillicores: shared ? null : 500,
      memoryBytes: shared ? null : 1073741824,
      volumeBytes: shared ? null : 10737418240,
      logicalStorageAllowanceBytes: shared ? 1073741824 : null,
      connectionLimit: 10,
      terms: String(form.get("terms")),
    });
    await hostingCall({
      action: "databases.plans.create",
      plan,
      idempotencyKey: String(form.get("idempotencyKey")),
    });
    revalidatePath("/infrastructure", "layout");
    return { message: "Database plan saved as disabled." };
  } catch (error) {
    return { error: hostingError(error).message };
  }
}

export async function revealDatabaseCredentials(
  databaseID: string,
): Promise<{ error?: string; credentials?: DatabaseCredentials }> {
  try {
    return {
      credentials: await hostingCall<DatabaseCredentials>({
        action: "databases.credentials.display",
        databaseID: resourceID.parse(databaseID),
      }),
    };
  } catch (error) {
    return { error: hostingError(error).message };
  }
}
