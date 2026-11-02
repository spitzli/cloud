import { formText } from "./form";
import { z } from "zod";
import {
  databaseCommandSchema,
  databasePlanSchema,
  databaseViewSchema,
} from "@webdock/hosting-contracts";
export { databaseCommandSchema, databasePlanSchema, databaseViewSchema };
export const planEntry = z.object({
  id: z.string(),
  plan: databasePlanSchema,
  availability: z.enum(["disabled", "pilot", "public"]),
  revision: z.number().int(),
});
export const offerSchema = z.object({
  id: z.string(),
  name: z.string(),
  expiresAt: z.string(),
  snapshot: databasePlanSchema.safeExtend({
    requiresInterruption: z.boolean().optional(),
    sourceDatabaseID: z.string().optional(),
    backupStatus: z.literal("not-configured").optional(),
    billing: z.enum(["sponsored", "monthly-net-fixed"]).optional(),
    billingStarts: z.literal("observed-ready").optional(),
    sourceRevision: z.number().int().optional(),
  }),
});
export const catalogSchema = z.array(z.unknown()).transform((rows) => {
  const plans: z.infer<typeof planEntry>[] = [];
  let unavailable = 0;
  for (const row of rows) {
    const parsed = planEntry.safeParse(row);
    if (parsed.success) plans.push(parsed.data);
    else unavailable++;
  }
  return { plans, unavailable };
});
export type PlanEntry = z.infer<typeof planEntry>;
export type Offer = z.infer<typeof offerSchema>;
export function acceptReviewedOffer(
  projectID: string,
  offer: {
    id: string;
    snapshot: { requiresInterruption?: boolean | undefined };
  },
  form: FormData,
) {
  if (form.get("confirmPrice") !== "yes")
    throw Error("Accept the displayed monthly price and terms.");
  if (
    offer.snapshot.requiresInterruption &&
    form.get("confirmInterruption") !== "yes"
  )
    throw Error(
      "Confirm the maintenance interruption before accepting this upgrade.",
    );
  return databaseCommandSchema.parse({
    action: "databases.acceptOrder",
    projectID,
    offerID: offer.id,
    idempotencyKey: `${offer.id}-accept-database`,
    ...(offer.snapshot.requiresInterruption
      ? { confirmInterruption: true }
      : {}),
  });
}
export function planFromForm(form: FormData) {
  const price = formText(form, "monthlyPrice");
  if (!/^\d+(\.\d{1,2})?$/.test(price))
    throw Error("Enter a monthly price with at most two decimal places.");
  const kind = formText(form, "kind");
  return databasePlanSchema.parse({
    kind,
    postgresMajor: 18,
    name: formText(form, "name").trim(),
    monthlyPriceCents: Math.round(Number(price) * 100),
    currency: "EUR",
    connectionLimit: 20,
    terms: formText(form, "terms").trim(),
    cpuMillicores: kind === "dedicated" ? Number(form.get("cpu")) : null,
    memoryBytes:
      kind === "dedicated" ? Number(form.get("memory")) * 1073741824 : null,
    volumeBytes:
      kind === "dedicated" ? Number(form.get("volume")) * 1073741824 : null,
    logicalStorageAllowanceBytes:
      kind === "shared" ? Number(form.get("volume")) * 1073741824 : null,
  });
}
