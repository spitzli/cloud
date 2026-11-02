import { z } from "zod";
const id = z.string().regex(/^[1-9][0-9]{0,18}$/);
const revision = z.string().regex(/^(0|[1-9][0-9]{0,18})$/);
const resourceState = z.enum(["pending", "ready", "needs_review"]);
export const serviceSchema = z.object({
  data: z.object({
    service: z.object({
      customerID: id,
      enabled: z.boolean(),
      state: z.enum([
        "disabled",
        "pending",
        "provisioning",
        "ready",
        "suspended",
        "needs_review",
      ]),
      revision,
    }),
    mailboxesAvailable: z.boolean(),
    sendingAvailable: z.boolean(),
    canActivate: z.boolean(),
    canSuspend: z.boolean(),
    canReconcile: z.boolean(),
    pendingReview: z.boolean(),
  }),
});
export type MailService = z.infer<typeof serviceSchema>["data"];
export const directorySchema = z.object({
  data: z.object({
    customerID: id,
    operator: z.boolean(),
    canManage: z.boolean(),
    canConfigure: z.boolean(),
    canRetryConfiguration: z.boolean().optional().default(false),
    configuration: z
      .object({
        revision,
        appliedRevision: revision,
        state: z
          .enum([
            "pending",
            "running",
            "succeeded",
            "superseded",
            "needs_review",
          ])
          .nullable(),
      })
      .nullable(),
    mailboxes: z.array(
      z.object({
        id,
        name: z.string(),
        address: z.string(),
        state: resourceState,
        enabled: z.boolean(),
        quotaBytes: revision,
        canOpen: z.boolean(),
      }),
    ),
    domains: z.array(
      z.object({
        id,
        name: z.string(),
        state: resourceState,
        dkimKeys: z.array(
          z.object({
            id: z.string(),
            selector: z.string(),
            publicKey: z.string(),
          }),
        ),
      }),
    ),
    verifiedDomains: z.array(z.object({ id, domain: z.string() })),
    people: z.array(
      z.object({ id: z.string(), name: z.string(), email: z.string() }),
    ),
    grants: z.array(
      z.object({
        mailboxID: id,
        subject: z.string(),
        enabled: z.boolean(),
        revision,
      }),
    ),
  }),
});
export type MailDirectory = z.infer<typeof directorySchema>["data"];
export const lifecycleSchema = z.enum(["activate", "suspend", "reconcile"]);
export type MailAction = z.infer<typeof lifecycleSchema>;
const quotaBytes = revision.pipe(
  z
    .string()
    .refine(
      (value) => BigInt(value) >= 1048576n && BigInt(value) <= 8589934592n,
      "Quota must be between 1048576 and 8589934592 bytes.",
    ),
);
export const mailboxCommandSchema = z.discriminatedUnion("action", [
  z.strictObject({ action: z.literal("domain.attach"), senderDomainID: id }),
  z.strictObject({
    action: z.literal("mailbox.create"),
    domainID: id,
    localPart: z
      .string()
      .max(64)
      .regex(/^[a-z0-9]+(?:[._+-][a-z0-9]+)*$/),
    name: z
      .string()
      .trim()
      .min(1)
      .max(80)
      .refine((value) => !/[\0\r\n]/.test(value)),
    quotaBytes,
  }),
  z.strictObject({ action: z.literal("grant"), mailboxID: id, subject: id }),
  z.strictObject({ action: z.literal("revoke"), mailboxID: id, subject: id }),
  z.strictObject({ action: z.literal("configuration.retry"), revision: id }),
]);
export type MailboxCommand = z.infer<typeof mailboxCommandSchema>;
export const mailboxChangeSchema = z.object({
  data: z.object({ ok: z.literal(true), id: id.optional() }),
});
export const commandSchema = z.discriminatedUnion("action", [
  z.strictObject({ action: z.literal("launchTenantWebmail"), customerID: id, mailboxID: id }),
  z.strictObject({
    action: z.literal("manageTenantMailboxes"),
    customerID: id,
    input: mailboxCommandSchema,
  }),
  z.strictObject({ action: z.literal("getTenantMailService"), customerID: id }),
  z.strictObject({ action: z.literal("getTenantMailboxes"), customerID: id }),
  z.strictObject({
    action: z.literal("manageTenantMailService"),
    customerID: id,
    input: z.strictObject({ action: lifecycleSchema, revision }),
  }),
]);
export type MailCommand = z.infer<typeof commandSchema>;
export function lifecycleResolved(
  value: MailService,
  previous: string,
): boolean {
  return (
    BigInt(value.service.revision) > BigInt(previous) &&
    ["ready", "suspended", "disabled", "needs_review"].includes(
      value.service.state,
    )
  );
}
