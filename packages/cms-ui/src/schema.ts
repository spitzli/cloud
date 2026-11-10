import {msgid} from '@spitzli/i18n';
import type { Field } from "payload";
import type { CMSField } from "./types";
const scalar = new Set([
  "text",
  "email",
  "textarea",
  "number",
  "checkbox",
  "select",
  "radio",
  "date",
  "slug",
  "relationship",
  "upload",
  "richText",
  "json",
  "code",
]);
const defaultLabels:Record<string,string>={title:msgid('Title'),description:msgid('Description'),question:msgid('Question'),answer:msgid('Answer'),contactEmail:msgid('Contact email'),name:msgid('Name'),slug:msgid('Slug'),alt:msgid('Alternative text'),source:msgid('Source')};
const label = (value: unknown, fallback: string) =>
  typeof value === "string"
    ? value
    : value && typeof value === "object"
      ? String(
          (value as Record<string, unknown>).en ||
            (value as Record<string, unknown>).de ||
            fallback,
        )
      : defaultLabels[fallback] || fallback.replace(/([a-z])([A-Z])/g, "$1 $2").replaceAll("-", " ");
export function cmsFields(fields: Field[]): CMSField[] {
  const result: CMSField[] = [];
  for (const original of fields) {
    const f = original as unknown as Record<string, any>; // The serialized UI subset spans Payload's discriminated field union.
    if (
      f.admin?.hidden ||
      (typeof f.name === "string" && f.name.startsWith("_")) ||
      ["id", "createdAt", "updatedAt", "_status"].includes(f.name) ||
      f.type === "ui" ||
      f.type === "join"
    )
      continue;
    if (f.type === "tabs") {
      for (const tab of f.tabs || [])
        result.push({
          type: tab.name ? "group" : "section",
          name: tab.name,
          label: label(tab.label, tab.name || "Content"),
          fields: cmsFields(tab.fields || []),
          localized: tab.localized,
        });
      continue;
    }
    if (["row", "collapsible"].includes(f.type)) {
      result.push({
        type: "section",
        label: label(f.label, ""),
        fields: cmsFields(f.fields || []),
      });
      continue;
    }
    if (!f.name || ["__proto__", "constructor", "prototype"].includes(f.name))
      continue;
    const out: CMSField = {
      name: f.name,
      type: f.type,
      label: label(f.label, f.name),
      required: Boolean(f.required),
      readOnly:
        Boolean(f.admin?.readOnly) ||
        f.type === "json" ||
        (!scalar.has(f.type) && !["array", "blocks", "group"].includes(f.type)),
      localized: Boolean(f.localized),
    };
    if (typeof f.admin?.description === "string")
      out.description = f.admin.description;
    for (const key of [
      "minRows",
      "maxRows",
      "min",
      "max",
      "maxLength",
    ] as const)
      if (typeof f[key] === "number") out[key] = f[key];
    if (f.defaultValue !== undefined && typeof f.defaultValue !== "function")
      out.defaultValue = f.defaultValue;
    if (f.fields) out.fields = cmsFields(f.fields);
    if (f.blocks)
      out.blocks = f.blocks
        .filter((b: unknown) => typeof b === "object")
        .map((b: Record<string, any>) => ({
          slug: b.slug,
          label: label(b.labels?.singular, b.slug),
          fields: cmsFields(b.fields || []),
        }));
    if (f.options)
      out.options = f.options.map((o: unknown) =>
        typeof o === "string"
          ? { label: o, value: o }
          : {
              label: label((o as any).label, (o as any).value),
              value: String((o as any).value),
            },
      );
    if (f.relationTo) out.relationTo = f.relationTo;
    if (f.hasMany) out.hasMany = true;
    result.push(out);
  }
  return result;
}
export function defaultData(fields: CMSField[]): Record<string, unknown> {
  const data: Record<string, unknown> = {};
  for (const f of fields) {
    if (f.type === "section") Object.assign(data, defaultData(f.fields || []));
    else if (f.name && !f.readOnly) {
      if (f.defaultValue !== undefined)
        data[f.name] = structuredClone(f.defaultValue);
      else if (f.type === "group") data[f.name] = defaultData(f.fields || []);
      else if (["array", "blocks"].includes(f.type)) data[f.name] = [];
      else if (f.type === "checkbox") data[f.name] = false;
    }
  }
  return data;
}
export class CMSInputError extends Error {
  constructor(
    message: string,
    public status = 400,
  ) {
    super(message);
  }
}
export function editableData(
  fields: CMSField[],
  input: unknown,
  row = false,
): Record<string, unknown> {
  if (!input || typeof input !== "object" || Array.isArray(input))
    throw new CMSInputError(msgid("Expected content fields."));
  const flattened = fields.flatMap((f) =>
    f.type === "section" ? flatten(f.fields || []) : [f],
  );
  const source = input as Record<string, unknown>,
    out: Record<string, unknown> = {};
  const allowed = new Set(
    flattened.filter((f) => f.name && !f.readOnly).map((f) => f.name),
  );
  if (row) {
    allowed.add("id");
    allowed.add("blockType");
  }
  for (const key of Object.keys(source))
    if (!allowed.has(key))
      throw new CMSInputError(`This field cannot be edited: ${key}`);
  if (row && source.id !== undefined) {
    if (
      !["string", "number"].includes(typeof source.id) ||
      String(source.id).length > 128
    )
      throw new CMSInputError(msgid("Invalid item ID."));
    out.id = source.id;
  }
  if (row && source.blockType !== undefined) out.blockType = source.blockType;
  for (const f of flattened) {
    if (!f.name || f.readOnly || !(f.name in source)) continue;
    const value = source[f.name];
    if (f.type === "group")
      out[f.name] = value === null ? null : editableData(f.fields || [], value);
    else if (f.type === "array" || f.type === "blocks") {
      if (!Array.isArray(value) || value.length > (f.maxRows ?? 500))
        throw new CMSInputError(`Invalid items in ${f.label}.`);
      out[f.name] = value.map((item) => {
        if (f.type === "blocks") {
          const block = f.blocks?.find((b) => b.slug === item?.blockType);
          if (!block) throw new CMSInputError(msgid("Unknown content block."));
          return editableData(block.fields, item, true);
        }
        return editableData(f.fields || [], item, true);
      });
    } else out[f.name] = value;
  }
  return out;
}
const flatten = (fields: CMSField[]): CMSField[] =>
  fields.flatMap((f) => (f.type === "section" ? flatten(f.fields || []) : [f]));
export function editorData(
  fields: CMSField[],
  input: Record<string, unknown>,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const f of flatten(fields)) {
    if (!f.name || f.readOnly || !(f.name in input)) continue;
    const value = input[f.name];
    if (f.type === "group" && value && typeof value === "object")
      out[f.name] = editorData(
        f.fields || [],
        value as Record<string, unknown>,
      );
    else if (["array", "blocks"].includes(f.type) && Array.isArray(value))
      out[f.name] = value.map((item) => ({
        ...("id" in item ? { id: item.id } : {}),
        ...(item.blockType ? { blockType: item.blockType } : {}),
        ...editorData(
          f.type === "blocks"
            ? f.blocks?.find((b) => b.slug === item.blockType)?.fields || []
            : f.fields || [],
          item,
        ),
      }));
    else out[f.name] = value;
  }
  return out;
}
