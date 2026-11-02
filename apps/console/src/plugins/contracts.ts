import { z } from "zod";

const label = z.string().min(1).max(160);
const key = z.string().regex(/^[a-zA-Z][a-zA-Z0-9_.-]{0,79}$/);
const scalar = z.union([
  z.string().max(4000),
  z.number(),
  z.boolean(),
  z.null(),
]);
const action = z
  .object({ id: key, label, destructive: z.boolean().optional() })
  .strict();
const field = z
  .object({
    name: key,
    label,
    // A secret is write-only: never pre-filled and never part of panel data.
    type: z.enum(["text", "email", "number", "select", "checkbox", "secret", "textarea"]),
    required: z.boolean().optional(),
    options: z
      .array(z.object({ value: z.string().max(160), label }).strict())
      .max(50)
      .optional(),
  })
  .strict()
  .refine(
    (value) => value.type !== "select" || Boolean(value.options?.length),
    "Select fields require options",
  );
const block = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("text"), text: z.string().max(8000) }).strict(),
  z
    .object({
      kind: z.literal("status"),
      label,
      state: z.enum(["healthy", "pending", "failed", "unknown"]),
      text: z.string().max(500),
    })
    .strict(),
  z
    .object({
      kind: z.literal("table"),
      title: label,
      columns: z.array(z.object({ key, label }).strict()).min(1).max(12),
      // Linked rows carry the resource id they open; it is not shown as a column.
      rowLink: z.literal("resource").optional(),
      rows: z
        .array(
          z.record(key, scalar).refine((row) => Object.keys(row).length <= 13),
        )
        .max(200),
    })
    .strict()
    .refine(
      (table) =>
        table.rowLink !== "resource" ||
        table.rows.every((row) => typeof row.resource_id === "string" && row.resource_id.length > 0),
      "Linked rows need a resource_id",
    ),
  // Credentials stay out of panel data: the value is fetched by a read action on click.
  z.object({ kind: z.literal("reveal"), label, action: key }).strict(),
  z
    .object({
      kind: z.literal("form"),
      title: label,
      fields: z
        .array(field)
        .min(1)
        .max(24)
        .refine(
          (fields) => new Set(fields.map((f) => f.name)).size === fields.length,
          "Duplicate fields",
        ),
      action,
    })
    .strict(),
  z
    .object({
      kind: z.literal("actions"),
      actions: z.array(action).min(1).max(12),
    })
    .strict(),
]);
export const panelSchema = z
  .object({
    version: z.literal(2),
    title: label,
    refreshSeconds: z.number().int().min(5).max(300).optional(),
    blocks: z.array(block).max(50),
  })
  .strict();
export type Panel = z.infer<typeof panelSchema>;
export type Action = z.infer<typeof action>;
export const revealSchema = z.object({ value: z.string().max(8000) }).strict();
export function parsePanel(input: unknown): Panel {
  if (((JSON.stringify(input) as string | undefined)?.length ?? 0) > 262144)
    throw new Error("Panel exceeds size limit");
  return panelSchema.parse(input);
}
const launchSchema = z
  .object({
    version: z.literal(1),
    installationId: key,
    url: z.url().max(2048),
    actions: z.array(key).max(64),
  })
  .strict();
export type Launch = z.infer<typeof launchSchema>;
export function parseLaunch(value: unknown, coreOrigins: string[]): Launch {
  if (coreOrigins.length === 0)
    throw new Error("Core origin policy is required");
  const blockedOrigins = coreOrigins.map((value) => new URL(value).origin);
  const launch = launchSchema.parse(value);
  const url = new URL(launch.url);
  if (
    url.protocol !== "https:" ||
    url.username ||
    url.password ||
    blockedOrigins.includes(url.origin)
  )
    throw new Error("Plugin UI must use a separate HTTPS origin");
  return launch;
}
const messageSchema = z
  .object({
    version: z.literal(1),
    type: z.literal("webdock.action"),
    requestId: key,
    installationId: key,
    action: key,
    input: z
      .record(key, scalar)
      .refine((value) => Object.keys(value).length <= 24),
  })
  .strict();
export type ActionMessage = z.infer<typeof messageSchema>;
export interface BridgePolicy {
  source: unknown;
  origin: string;
  installationId: string;
  actions: readonly string[];
  active: boolean;
}
export function validateActionMessage(
  event: { source: unknown; origin: string; data: unknown },
  policy: BridgePolicy,
): ActionMessage | null {
  if (
    !policy.active ||
    !policy.source ||
    event.source !== policy.source ||
    event.origin !== policy.origin
  )
    return null;
  try {
    if (
      ((JSON.stringify(event.data) as string | undefined)?.length ?? 0) > 32768
    )
      return null;
    const message = messageSchema.parse(event.data);
    return message.installationId === policy.installationId &&
      policy.actions.includes(message.action)
      ? message
      : null;
  } catch {
    return null;
  }
}
