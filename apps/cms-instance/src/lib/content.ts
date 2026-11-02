import type {
  Access,
  CollectionConfig,
  GlobalConfig,
  AuthStrategy,
} from "payload";
import { APIError } from "payload";
import { createHash } from "node:crypto";
import { msgid } from "@webdock/i18n";
import {
  protectContent,
  protectUsers,
  canManageUsers,
} from "@webdock/instance-kit";
import { authSubjectField } from "@webdock/payload-sso";
import type { CMSOptions } from "@webdock/cms-ui/types";
export const modules: CMSOptions["modules"] = [
  {
    slug: "pages",
    kind: "collection",
    label: "Pages",
    titleField: "title",
    previewPath: "/{locale}/{slug}",
    draftPreviewPath: "/preview/{locale}/{slug}",
  },
  { slug: "media", kind: "collection", label: "Media", titleField: "alt" },
  { slug: "settings", kind: "global", label: "Site settings" },
];
export const publicOrAccount: Access = ({ req }) =>
  req.user?.collection === "users"
    ? true
    : { _status: { equals: "published" } };
export const pages: CollectionConfig = protectContent({
  slug: "pages",
  admin: { useAsTitle: "title" },
  // Installed Payload derives localized publication status from localized fields.
  versions: { drafts: true, maxPerDoc: 30 },
  access: { read: publicOrAccount },
  fields: [
    {
      name: "title",
      type: "text",
      required: true,
      localized: true,
      maxLength: 200,
    },
    {
      name: "slug",
      type: "text",
      required: true,
      unique: true,
      index: true,
      maxLength: 120,
      validate: (value: unknown) =>
        (typeof value === "string" &&
          /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(value)) ||
        "Use lowercase words separated by hyphens.",
    },
    {
      name: "content",
      type: "blocks",
      localized: true,
      maxRows: 100,
      blocks: [
        {
          slug: "text",
          labels: { singular: "Text", plural: "Text blocks" },
          fields: [
            { name: "heading", type: "text", maxLength: 200 },
            {
              name: "text",
              type: "textarea",
              required: true,
              maxLength: 50000,
            },
          ],
        },
        {
          slug: "image",
          labels: { singular: "Image", plural: "Images" },
          fields: [
            {
              name: "image",
              type: "upload",
              relationTo: "media",
              required: true,
            },
            { name: "caption", type: "text", maxLength: 500 },
          ],
        },
      ],
    },
    {
      name: "seo",
      type: "group",
      localized: true,
      fields: [
        { name: "title", type: "text", maxLength: 200 },
        { name: "description", type: "textarea", maxLength: 500 },
      ],
    },
  ],
});
export const media = (directory: string): CollectionConfig =>
  protectContent({
    slug: "media",
    trash: true,
    hooks: {
      beforeValidate: [
        ({ data }) => ({ ...data, prefix: "", _objectKey: null }),
      ],
      beforeChange: [
        ({ data, originalDoc, req }) => {
          if (
            Object.prototype.hasOwnProperty.call(data ?? {}, "deletedAt") &&
            (data.deletedAt ?? null) !== (originalDoc?.deletedAt ?? null) &&
            !canManageUsers({ req })
          )
            throw new APIError(
              msgid("An administrator is required to remove or restore media."),
              403,
            );
          return data;
        },
      ],
    },
    admin: { useAsTitle: "alt" },
    access: { read: () => true },
    upload: {
      staticDir: directory,
      mimeTypes: ["image/png", "image/jpeg", "image/webp", "image/avif"],
      disableLocalStorage: false,
      filesRequiredOnCreate: true,
    },
    fields: [
      {
        name: "contentSha256",
        type: "text",
        hidden: true,
        admin: { hidden: true },
        access: { create: () => false, update: () => false },
        hooks: {
          beforeChange: [
            ({ req, originalDoc }) =>
              req.file
                ? createHash("sha256").update(req.file.data).digest("hex")
                : (originalDoc?.contentSha256 ?? null),
          ],
        },
      },
      {
        name: "alt",
        type: "text",
        required: true,
        localized: true,
        maxLength: 500,
      },
    ],
  });
export const settings: GlobalConfig = protectContent({
  slug: "settings",
  // Installed Payload derives localized publication status from localized fields.
  versions: { drafts: true, max: 30 },
  access: { read: ({ req }) => Boolean(req.user) },
  fields: [
    {
      name: "title",
      type: "text",
      localized: true,
      required: true,
      maxLength: 200,
    },
    { name: "description", type: "textarea", localized: true, maxLength: 1000 },
    {
      name: "navigation",
      type: "array",
      localized: true,
      maxRows: 30,
      fields: [
        { name: "label", type: "text", required: true, maxLength: 100 },
        {
          name: "page",
          type: "relationship",
          relationTo: "pages",
          required: true,
        },
      ],
    },
  ],
});
export function users(strategies: AuthStrategy[] = []): CollectionConfig {
  process.env.WEBDOCK_SSO_ENFORCE = "true";
  return protectUsers(
    {
      slug: "users",
      auth: {
        disableLocalStrategy: { enableFields: true, optionalPassword: true },
        strategies,
      },
      fields: [
        authSubjectField,
        { name: "name", type: "text" },
        {
          name: "disabled",
          type: "checkbox",
          defaultValue: false,
          access: { create: () => false, update: () => false },
        },
      ],
    },
    "unassigned-system-operator@subject.invalid",
  );
}
