import { z } from "zod";
import { recordID } from "@spitzli/core/registry/contracts";
const text = z.string().trim().max(160);
const notes = z.string().trim().max(3000).nullable().optional();
const url = z
  .string()
  .url()
  .refine((v) => {
    const u = new URL(v);
    return u.protocol === "https:" && !u.username && !u.password;
  }, "Use an HTTPS URL without credentials")
  .nullable()
  .optional();
export const instanceInput = z
  .object({
    project: recordID,
    label: text.min(1),
    adminURL: url.unwrap().unwrap(),
    schemaName: z.string().regex(/^[a-z][a-z0-9_]{0,62}$/),
    providerProjectID: text.min(1),
    provider: z.enum(["vercel", "other"]),
    template: z.enum([
      "webdock-landing",
      "spitzli-portfolio",
      "stall-business",
      "custom",
    ]),
    payloadVersion: z.string().max(64).nullable().optional(),
    status: z.enum(["active", "suspended", "retired"]),
    notes,
  })
  .strict();
