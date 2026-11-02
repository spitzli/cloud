import { z } from "zod";
import semver from "semver";
import {
  capabilitySchema,
  readCapabilities,
  reservedPluginIDs,
  serviceNames,
  slotNames,
  pluginLimits,
} from "./capabilities.js";
import { PluginError } from "./errors.js";
const slug = z
  .string()
  .min(1)
  .max(64)
  .regex(/^[a-z][a-z0-9-]*$/);
const name = z
  .string()
  .min(1)
  .max(64)
  .regex(/^[a-z][a-z0-9_.-]*$/);
const fieldName = z.string().min(1).max(64).regex(/^[a-z][a-zA-Z0-9_.-]*$/)
  .refine((v) => !v.split(/[.-]/).some((p) => ["__proto__", "constructor", "prototype"].includes(p)));
const exactVersion = z
  .string()
  .max(40)
  .refine((v) => semver.valid(v) === v, "Expected exact SemVer");
const unique = <T>(items: readonly T[]) => new Set(items).size === items.length;
const uniqueNames = <T extends { name: string }>(items: readonly T[]) =>
  unique(items.map((x) => x.name));
const caps = z
  .array(capabilitySchema)
  .max(32)
  .refine(unique, "Duplicate capability");
const path = z
  .string()
  .max(240)
  .regex(/^\/ui\/(?:[a-zA-Z0-9_-]+\/)*[a-zA-Z0-9_-]+(?:\.[a-zA-Z0-9_-]+)?$/);
const positive = z.number().int().min(1).max(2147483647);
const literal = z.union([
  z.string().max(4096),
  z.number().finite(),
  z.boolean(),
  z.null(),
]);
// Bounded, local schemas only: no references, regexes or executable extensions.
export type ValueSchema = {
  type:
    "object" | "array" | "string" | "number" | "integer" | "boolean" | "null";
  properties?: Record<string, ValueSchema>;
  required?: string[];
  additionalProperties?: false;
  items?: ValueSchema;
  enum?: Array<string | number | boolean | null>;
  default?: string | number | boolean | null;
  minLength?: number;
  maxLength?: number;
  minimum?: number;
  maximum?: number;
  minItems?: number;
  maxItems?: number;
  description?: string;
};
function schemaAtDepth(depth: number): z.ZodType<ValueSchema> {
  const nested: z.ZodType<ValueSchema> =
    depth < 3 ? schemaAtDepth(depth + 1) : z.never();
  return z
    .strictObject({
      type: z.enum([
        "object",
        "array",
        "string",
        "number",
        "integer",
        "boolean",
        "null",
      ]),
      properties: z.record(fieldName, nested).optional(),
      required: z.array(fieldName).max(64).refine(unique).optional(),
      additionalProperties: z.literal(false).optional(),
      items: nested.optional(),
      enum: z.array(literal).min(1).max(64).optional(),
      default: literal.optional(),
      minLength: z.number().int().nonnegative().max(65536).optional(),
      maxLength: z.number().int().nonnegative().max(65536).optional(),
      minimum: z.number().finite().optional(),
      maximum: z.number().finite().optional(),
      minItems: z.number().int().nonnegative().max(100).optional(),
      maxItems: z.number().int().nonnegative().max(100).optional(),
      description: z.string().max(2000).optional(),
    })
    .superRefine((s, ctx) => {
      const issue = (message: string) =>
        ctx.addIssue({ code: "custom", message });
      if (s.properties && Object.keys(s.properties).length > 64)
        issue("Too many properties");
      if (
        s.required?.some(
          (n) => !s.properties || !Object.hasOwn(s.properties, n),
        )
      )
        issue("Unknown required property");
      if (s.type === "array" && !s.items) issue("Array items must be bounded");
      for (const [min, max] of [
        [s.minLength, s.maxLength],
        [s.minimum, s.maximum],
        [s.minItems, s.maxItems],
      ])
        if (min !== undefined && max !== undefined && min > max)
          issue("Invalid schema bounds");
      const matches = (v: unknown) =>
        s.type === "null"
          ? v === null
          : s.type === "integer"
            ? typeof v === "number" && Number.isSafeInteger(v)
            : s.type === "number"
              ? typeof v === "number"
              : typeof v === s.type;
      if (s.default !== undefined && !matches(s.default))
        issue("Default does not match schema type");
      if (s.enum?.some((v) => !matches(v)))
        issue("Enum does not match schema type");
    });
}
export const valueSchema = schemaAtDepth(0);
export const columnSchema = z
  .strictObject({
    name,
    type: z.enum([
      "string",
      "integer",
      "boolean",
      "timestamp",
      "json",
      "decimal",
    ]),
    nullable: z.boolean(),
    default: literal.optional(),
  })
  .superRefine((column, ctx) => {
    const value = column.default;
    if (value === undefined) return;
    const valid =
      value === null
        ? column.nullable
        : column.type === "string"
          ? typeof value === "string"
          : column.type === "integer"
            ? typeof value === "number" && Number.isSafeInteger(value)
            : column.type === "boolean"
              ? typeof value === "boolean"
              : column.type === "timestamp"
                ? typeof value === "string" &&
                  z.iso.datetime({ offset: true }).safeParse(value).success
                : column.type === "decimal"
                  ? typeof value === "string" &&
                    /^-?(0|[1-9][0-9]*)(\.[0-9]+)?$/.test(value)
                  : true;
    if (!valid)
      ctx.addIssue({
        code: "custom",
        path: ["default"],
        message: "Default does not match column type/nullability",
      });
  });
const columns = z.array(name).min(1).max(16).refine(unique);
const indexSchema = z.strictObject({ name, columns, unique: z.boolean() });
const foreignKeySchema = z.strictObject({
  name,
  columns,
  references: z.strictObject({ table: name, columns }),
});
export const tableSchema = z
  .strictObject({
    name,
    columns: z.array(columnSchema).min(1).max(64).refine(uniqueNames),
    primaryKey: columns,
    indexes: z.array(indexSchema).max(32).refine(uniqueNames),
    foreignKeys: z.array(foreignKeySchema).max(32).refine(uniqueNames),
  })
  .superRefine((t, ctx) => {
    const known = new Set(t.columns.map((c) => c.name));
    for (const key of [
      t.primaryKey,
      ...t.indexes.map((i) => i.columns),
      ...t.foreignKeys.map((i) => i.columns),
    ])
      if (key.some((c) => !known.has(c)))
        ctx.addIssue({ code: "custom", message: "Unknown key column" });
    if (t.primaryKey.some((n) => t.columns.find((c) => c.name === n)?.nullable))
      ctx.addIssue({ code: "custom", message: "Nullable primary key" });
    for (const key of t.foreignKeys)
      if (key.columns.length !== key.references.columns.length)
        ctx.addIssue({ code: "custom", message: "Foreign key arity mismatch" });
  });
export const migrationStepSchema = z.discriminatedUnion("op", [
  z.strictObject({ op: z.literal("createTable"), table: tableSchema }),
  z.strictObject({
    op: z.literal("addNullableColumn"),
    table: name,
    column: columnSchema.safeExtend({ nullable: z.literal(true) }),
  }),
  z.strictObject({
    op: z.literal("createIndex"),
    table: name,
    index: indexSchema,
  }),
  z.strictObject({
    op: z.literal("addForeignKey"),
    table: name,
    foreignKey: foreignKeySchema,
  }),
]);
export const migrationSchema = z.strictObject({
  version: positive,
  checksum: z.string().regex(/^[a-f0-9]{64}$/),
  steps: z.array(migrationStepSchema).min(1).max(64),
});
export const dependencySchema = z.strictObject({
  id: slug,
  version: exactVersion,
});
const services = z
  .array(z.strictObject({ name: z.enum(serviceNames), version: z.literal(1) }))
  .max(16)
  .refine(uniqueNames);
export const extensionSchema = z.strictObject({
  protocolVersion: z.literal(1),
  sdkVersion: z.literal(1),
  dataProtocolVersion: z.literal(1),
  coreVersion: z
    .string()
    .min(1)
    .max(120)
    .refine((v) => semver.validRange(v) !== null, "Invalid core range"),
  image: z
    .string()
    .max(500)
    .regex(
      /^[a-z0-9]+(?:[.-][a-z0-9]+)*(?::[0-9]{1,5})?\/(?:[a-z0-9]+(?:[._-][a-z0-9]+)*\/)*[a-z0-9]+(?:[._-][a-z0-9]+)*@sha256:[a-f0-9]{64}$/,
    ),
  dependencies: z
    .array(dependencySchema)
    .max(32)
    .refine((v) => unique(v.map((d) => d.id)), "Duplicate dependency"),
  capabilities: caps,
  schemaVersion: positive,
  readableSchema: z.strictObject({ min: positive, max: positive }),
  migrations: z.array(migrationSchema).max(256),
  tables: z.array(tableSchema).max(32).refine(uniqueNames),
  services: z.strictObject({ required: services, optional: services }),
  configuration: valueSchema,
  actions: z
    .array(
      z.strictObject({
        name,
        intent: z.enum(["read", "write"]),
        capabilities: caps,
        input: valueSchema.optional(),
        output: valueSchema.optional(),
        storage: z.enum(["upload", "download"]).optional(),
        httpBinding: name.optional(),
        destructive: z.boolean().optional(),
      }),
    )
    .max(64)
    .refine(uniqueNames),
  jobs: z
    .array(
      z.strictObject({
        name,
        intervalSeconds: z.number().int().min(60).max(2147483647).optional(),
        capabilities: caps,
        input: valueSchema.optional(),
      }),
    )
    .max(32)
    .refine(uniqueNames),
  navigation: z
    .array(z.strictObject({ id: slug, label: z.string().min(1).max(80), path }))
    .max(32)
    .refine((v) => unique(v.map((n) => n.id))),
  slots: z
    .array(
      z.strictObject({
        id: slug,
        slot: z.enum(slotNames),
        order: z.number().int().min(-1000).max(1000),
        path,
        capabilities: caps,
        label: z.string().max(80).optional(),
        action: name.optional(),
      }),
    )
    .max(32)
    .refine((v) => unique(v.map((s) => s.id))),
  events: z
    .array(
      z.strictObject({
        name,
        version: positive,
        schema: valueSchema,
        handlers: z.array(name).min(1).max(32).refine(unique),
      }),
    )
    .max(32)
    .refine(uniqueNames),
  secrets: z.array(name).max(32).refine(unique),
  httpBindings: z.array(name).max(32).refine(unique),
  // Paths are local to /v1/plugins/:pluginID/projects/:projectID/api.
  // Exact routes only: parameters belong to the action input schema.
  api: z
    .array(
      z.strictObject({
        method: z.enum(["GET", "POST", "PUT", "PATCH", "DELETE"]),
        path: z
          .string()
          .max(160)
          .regex(/^\/(?:[a-z][a-z0-9-]*\/)*[a-z][a-z0-9-]*$/),
        action: name,
      }),
    )
    .max(64)
    .optional(),
  // Host prefixes names with the plugin ID; plugins cannot replace core tools.
  mcp: z
    .array(
      z.strictObject({
        name: z
          .string()
          .max(64)
          .regex(/^[a-z][a-z0-9_]*$/),
        description: z.string().min(1).max(1000),
        action: name,
      }),
    )
    .max(64)
    .optional(),
});
const displayURL = z
  .string()
  .max(500)
  .url()
  .refine((v) => ["http:", "https:"].includes(new URL(v).protocol));
export const manifestSchema = z
  .strictObject({
    name: slug.refine(
      (v) => !(reservedPluginIDs as readonly string[]).includes(v),
      "Reserved plugin id",
    ),
    version: exactVersion,
    kind: z.literal("extension"),
    description: z.string().max(280).optional(),
    category: z.string().max(40).optional(),
    tags: z.array(z.string().max(30)).max(16).optional(),
    license: z.string().max(40).optional(),
    icon: z.string().max(500).optional(),
    screenshots: z
      .array(
        z.strictObject({
          url: z.string().min(1).max(500),
          caption: z.string().max(200).optional(),
          alt: z.string().max(200).optional(),
        }),
      )
      .max(12)
      .optional(),
    readme: z.string().max(500).optional(),
    readmes: z
      .record(z.string().regex(/^[a-z]{2}(-[A-Z]{2})?$/), z.string().max(500))
      .optional(),
    documentation_url: displayURL.optional(),
    homepage: displayURL.optional(),
    support: z
      .strictObject({
        email: z.string().max(254).optional(),
        issues_url: displayURL.optional(),
      })
      .optional(),
    min_runtime_version: z.string().max(40).optional(),
    ...extensionSchema.shape,
  })
  .superRefine((m, ctx) => {
    const x = m;
    const issue = (message: string) =>
      ctx.addIssue({ code: "custom", message });
    if (x.dependencies.some((d) => d.id === m.name)) issue("Self dependency");
    if (
      x.readableSchema.min > x.readableSchema.max ||
      x.schemaVersion < x.readableSchema.min ||
      x.schemaVersion > x.readableSchema.max
    )
      issue("Invalid readable schema range");
    let prior = 0;
    for (const migration of x.migrations) {
      if (
        migration.version !== prior + 1 ||
        migration.version > x.schemaVersion
      )
        issue("Migrations must be ordered and contiguous");
      prior = migration.version;
    }
    if (x.tables.length && prior !== x.schemaVersion)
      issue("Missing schema migration chain");
    if (
      !unique(
        [...x.services.required, ...x.services.optional].map((s) => s.name),
      )
    )
      issue("Duplicate service");
    for (const a of [...x.actions, ...x.jobs, ...x.slots])
      if (a.capabilities.some((c) => !x.capabilities.includes(c)))
        issue("Undeclared capability");
    for (const a of x.actions) {
      if (
        a.httpBinding &&
        (!x.httpBindings.includes(a.httpBinding) ||
          !a.capabilities.length ||
          a.capabilities.some(
            (c) => c !== "plugin.http.read" && c !== "plugin.http.request",
          ) ||
          !a.capabilities.includes(
            a.intent === "write" ? "plugin.http.request" : "plugin.http.read",
          ))
      )
        issue(
          "Website action requires a declared HTTP binding and only matching HTTP capabilities",
        );
      if (
        a.intent === "read" &&
        a.capabilities.some((c) => !readCapabilities.includes(c))
      )
        issue("Read action requests mutation capability");
      if (
        a.storage === "upload" &&
        (a.intent !== "write" ||
          !a.capabilities.includes("plugin.storage.write"))
      )
        issue("Upload action requires storage write");
      if (
        a.storage === "download" &&
        !a.capabilities.includes("plugin.storage.read")
      )
        issue("Download action requires storage read");
    }
    for (const event of x.events)
      if (event.handlers.some((h) => !x.jobs.some((j) => j.name === h)))
        issue("Unknown event handler");
    for (const slot of x.slots)
      if (slot.action && !x.actions.some((a) => a.name === slot.action))
        issue("Unknown slot action");
    if (!unique((x.api ?? []).map((r) => `${r.method} ${r.path}`)))
      issue("Duplicate API route");
    if (!unique((x.mcp ?? []).map((t) => t.name))) issue("Duplicate MCP tool");
    for (const route of x.api ?? []) {
      const action = x.actions.find((a) => a.name === route.action);
      if (!action) issue("Unknown API action");
      if (route.method === "GET" && action?.intent !== "read")
        issue("GET requires a read action");
    }
    for (const tool of x.mcp ?? [])
      if (!x.actions.some((a) => a.name === tool.action))
        issue("Unknown MCP action");
    for (const t of x.tables)
      for (const fk of t.foreignKeys) {
        const target = x.tables.find((t) => t.name === fk.references.table);
        if (
          !target ||
          ![
            target.primaryKey,
            ...target.indexes.filter((i) => i.unique).map((i) => i.columns),
          ].some(
            (c) => JSON.stringify(c) === JSON.stringify(fk.references.columns),
          )
        )
          issue("Foreign key must reference a declared unique key");
      }
  });
export type PluginManifest = z.infer<typeof manifestSchema>;
export type PluginDependency = z.infer<typeof dependencySchema>;
function checkJSON(value: unknown, depth = 0): void {
  if (depth > 24)
    throw new PluginError("invalid_input", "Manifest nesting is too deep");
  if (
    value === null ||
    typeof value === "string" ||
    typeof value === "boolean" ||
    (typeof value === "number" && Number.isFinite(value))
  )
    return;
  if (Array.isArray(value)) {
    for (const item of value) checkJSON(item, depth + 1);
    return;
  }
  if (
    typeof value !== "object" ||
    Object.getPrototypeOf(value) !== Object.prototype
  )
    throw new PluginError("invalid_input", "Expected plain JSON");
  for (const [key, child] of Object.entries(value)) {
    if (["__proto__", "constructor", "prototype", "$ref", "$id"].includes(key))
      throw new PluginError("invalid_input", "Forbidden manifest key");
    checkJSON(child, depth + 1);
  }
}
export function parsePluginManifest(input: unknown): PluginManifest {
  checkJSON(input);
  if (
    new TextEncoder().encode(JSON.stringify(input)).byteLength >
    pluginLimits.manifestBytes
  )
    throw new PluginError("invalid_input", "Manifest exceeds 256 KiB");
  const result = manifestSchema.safeParse(input);
  if (!result.success)
    throw new PluginError(
      "invalid_input",
      result.error.issues
        .map((i) => `${i.path.join(".")}: ${i.message}`)
        .join(";"),
    );
  return result.data;
}
