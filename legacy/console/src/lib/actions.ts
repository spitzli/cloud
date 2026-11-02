"use server";
import { RegistryError as APIError } from "@webdock/api-client/errors";
import { ZodError } from "zod";
import {
  writeCustomer,
  writeProject,
  writeInstance,
  setArchived,
} from "./registry-remote";
import { redirect } from "next/navigation";
import { revalidatePath } from "next/cache";
import { requireRegistryOperator as requireOperator } from "./registry-page";
export type FormState = {
  error?: string;
  success?: string;
  values?: Record<string, string>;
};
const values = (form: FormData) =>
  Object.fromEntries(
    [...form.entries()]
      .filter(([, v]) => typeof v === "string")
      .map(([k, v]) => [k, String(v).trim()]),
  );
const fail = (error: unknown, data: Record<string, string>): FormState => ({
  error:
    error instanceof ZodError
      ? error.issues
          .map((issue) => `${issue.path.join(".")}: ${issue.message}`)
          .join("; ")
      : error instanceof APIError && error.isPublic
        ? error.message
        : "The change could not be saved. Please try again.",
  values: data,
});
const validID = (id: string) =>
  /^[1-9][0-9]{0,18}$/.test(id) && BigInt(id) <= 9223372036854775807n;
export async function saveCustomer(
  id: string | null,
  _state: FormState,
  form: FormData,
): Promise<FormState> {
  const { payload, user } = await requireOperator();
  const raw = values(form);
  let savedID: string;
  try {
    if (id && !validID(id)) throw new APIError("Invalid customer.", 400);
    const data = {
      name: raw.name,
      customerType: raw.customerType || undefined,
      firstName: raw.firstName || null,
      lastName: raw.lastName || null,
      companyName: raw.companyName || null,
      phone: raw.phone || null,
      addressLine1: raw.addressLine1 || null,
      addressLine2: raw.addressLine2 || null,
      postalCode: raw.postalCode || null,
      city: raw.city || null,
      region: raw.region || null,
      country: raw.country || null,
      contactName: raw.contactName || null,
      contactEmail: raw.contactEmail || null,
      notes: raw.notes || null,
    };
    const saved = await writeCustomer({ payload, user }, id, data);
    savedID = saved.id;
  } catch (e) {
    return fail(e, raw);
  }
  revalidatePath("/customers");
  if (id) {
    revalidatePath("/customers/" + savedID);
    return { success: "Customer saved." };
  }
  redirect("/customers/" + savedID);
}
export async function saveProject(
  id: string | null,
  _state: FormState,
  form: FormData,
): Promise<FormState> {
  const { payload, user } = await requireOperator();
  const raw = values(form);
  let savedID: string;
  try {
    if (id && !validID(id)) throw new APIError("Invalid project.", 400);
    const data = {
      name: raw.name,
      customer: raw.customer,
      url: raw.url || null,
      repositoryURL: raw.repositoryURL || null,
      notes: raw.notes || null,
    };
    const saved = await writeProject({ payload, user }, id, data);
    savedID = saved.id;
  } catch (e) {
    return fail(e, raw);
  }
  revalidatePath("/");
  if (id) {
    revalidatePath("/projects/" + savedID);
    return { success: "Project saved." };
  }
  redirect("/projects/" + savedID);
}
export async function archiveRecord(
  collection: "customers" | "projects",
  id: string,
  _state: FormState,
  form: FormData,
): Promise<FormState> {
  const { payload, user } = await requireOperator();
  try {
    if (!validID(id)) throw new APIError("Invalid record.", 400);
    await setArchived(
      { payload, user },
      collection,
      id,
      form.get("restore") !== "true",
    );
  } catch (e) {
    return fail(e, {});
  }
  revalidatePath(collection === "projects" ? "/" : "/customers");
  revalidatePath(`/${collection}/${id}`);
  return {};
}
export async function saveInstance(
  projectID: string,
  instanceID: string | null,
  _state: FormState,
  form: FormData,
): Promise<FormState> {
  const { payload, user } = await requireOperator();
  const raw = values(form);
  try {
    if (!validID(projectID) || (instanceID && !validID(instanceID)))
      throw new APIError("Invalid project.", 400);
    if (!instanceID && raw.confirmExisting !== "yes")
      throw new APIError("Confirm that this CMS is already deployed.", 400);
    const data = {
      project: projectID,
      label: raw.label,
      adminURL: raw.adminURL,
      schemaName: raw.schemaName,
      providerProjectID: raw.providerProjectID,
      provider: "vercel" as const,
      template: raw.template as
        "webdock-landing" | "spitzli-portfolio" | "stall-business" | "custom",
      payloadVersion: raw.payloadVersion || null,
      status: raw.status as "active" | "suspended" | "retired",
      notes: raw.notes || null,
    };
    await writeInstance(
      { payload, user },
      instanceID,
      data,
      raw.confirmExisting === "yes",
    );
  } catch (e) {
    return fail(e, raw);
  }
  revalidatePath("/");
  revalidatePath("/projects/" + projectID);
  return { success: "CMS connection saved." };
}
