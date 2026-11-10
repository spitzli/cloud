import {
  type Access,
  type AccessResult,
  APIError,
  type CollectionConfig,
  type GlobalConfig,
  type PayloadRequest,
} from "payload";

type Account = {
  id?: number | string;
  email?: string;
  role?: string;
  collection?: string;
};
const actor = (req: PayloadRequest) => req.user as Account | null;
export const isOperator = ({ req }: { req: PayloadRequest }) =>
  actor(req)?.collection === "users" && actor(req)?.role === "operator";
export const canManageUsers = ({ req }: { req: PayloadRequest }) =>
  actor(req)?.collection === "users" &&
  ["operator", "admin"].includes(actor(req)?.role || "");
export const canEditContent = ({ req }: { req: PayloadRequest }) =>
  actor(req)?.collection === "users" &&
  ["operator", "admin", "editor"].includes(actor(req)?.role || "");
const customerRecord: Access = ({ req }): AccessResult =>
  isOperator({ req })
    ? true
    : canManageUsers({ req })
      ? { role: { not_equals: "operator" } }
      : actor(req)?.collection === "users"
        ? { id: { equals: actor(req)?.id } }
        : false;

/** Visible system operator account. Customers cannot modify/delete it or grant this role. */
export function protectUsers(
  collection: CollectionConfig,
  operatorEmail: string,
): CollectionConfig {
  const centrallyManaged = process.env.SPITZLI_SSO_ENFORCE === "true";
  // Private, single-use markers: only real Payload recovery operations can set them.
  // They are never accepted from req.context or a caller-supplied request body.
  const recoveryRequests = new WeakMap<
    PayloadRequest,
    | { operation: "forgotPassword"; email: string }
    | { operation: "resetPassword"; token: string }
  >();
  const protectedRecord = (doc: Account) =>
    doc.role === "operator" ||
    doc.email?.toLowerCase() === operatorEmail.toLowerCase();
  return {
    ...collection,
    versions: false,
    admin: {
      ...collection.admin,
      ...(centrallyManaged ? { hidden: true } : {}),
    },
    access: {
      ...collection.access,
      admin: ({ req }) => actor(req)?.collection === "users",
      create: centrallyManaged ? () => false : canManageUsers,
      read: ({ req }): AccessResult =>
        canManageUsers({ req })
          ? true
          : actor(req)?.collection === "users"
            ? { id: { equals: actor(req)?.id } }
            : false,
      update: centrallyManaged ? () => false : customerRecord,
      delete: centrallyManaged
        ? () => false
        : ({ req }): AccessResult =>
            canManageUsers({ req })
              ? {
                  and: [
                    { role: { not_equals: "operator" } },
                    { email: { not_equals: operatorEmail } },
                  ],
                }
              : false,
      unlock: centrallyManaged ? () => false : customerRecord,
    },
    fields: [
      ...collection.fields.filter((f) => !("name" in f && f.name === "role")),
      {
        name: "role",
        type: "select",
        required: true,
        defaultValue: "editor",
        saveToJWT: true,
        options: [
          { label: "System operator (Webdock)", value: "operator" },
          { label: "Customer administrator", value: "admin" },
          { label: "Editor", value: "editor" },
          { label: "Read only", value: "reader" },
        ],
        access: { create: isOperator, update: isOperator },
        admin: {
          description:
            "The system operator is managed by Webdock, not by customer administrators.",
        },
      },
    ],
    hooks: {
      ...collection.hooks,
      beforeOperation: [
        ({ operation, req, args }) => {
          if (operation === "forgotPassword")
            recoveryRequests.set(req, {
              operation,
              email: args.data.email.toLowerCase().trim(),
            });
          else if (operation === "resetPassword")
            recoveryRequests.set(req, { operation, token: args.data.token });
          if (
            operation === "create" &&
            !req.user &&
            !req.context.instanceImport &&
            !req.context.bootstrap
          )
            throw new APIError(
              "Accounts are provisioned by an administrator.",
              403,
            );
        },
        ...(collection.hooks?.beforeOperation || []),
      ],
      beforeValidate: [
        ({ data, originalDoc, req }) => {
          const recovery = recoveryRequests.get(req);
          recoveryRequests.delete(req);
          if (req.context.instanceImport || req.context.bootstrap) return data;
          if (
            recovery?.operation === "forgotPassword" &&
            originalDoc?.email?.toLowerCase() === recovery.email &&
            typeof data?.resetPasswordToken === "string" &&
            typeof data?.resetPasswordExpiration === "string" &&
            Object.keys(data).every(
              (key) =>
                [
                  "resetPasswordToken",
                  "resetPasswordExpiration",
                  "updatedAt",
                ].includes(key) ||
                JSON.stringify(data[key]) === JSON.stringify(originalDoc[key]),
            )
          )
            return data;
          // Payload validates the token and expiry before this direct recovery hook.
          // Ordinary update operations always have originalDoc and cannot use this path.
          if (
            recovery?.operation === "resetPassword" &&
            !originalDoc &&
            data?.resetPasswordToken === recovery.token
          )
            return data;
          if (
            !isOperator({ req }) &&
            ((originalDoc && protectedRecord(originalDoc)) ||
              data?.role === "operator" ||
              data?.email?.toLowerCase() === operatorEmail.toLowerCase())
          )
            throw new APIError(
              "The system operator account is protected.",
              403,
            );
          if (
            originalDoc &&
            protectedRecord(originalDoc) &&
            data?.role &&
            data.role !== "operator"
          )
            throw new APIError(
              "The system operator role cannot be removed.",
              403,
            );
          return data;
        },
        ...(collection.hooks?.beforeValidate || []),
      ],
      afterOperation: [
        ({ operation, req, result }) => {
          if (operation === "forgotPassword" || operation === "resetPassword")
            recoveryRequests.delete(req);
          return result;
        },
        ...(collection.hooks?.afterOperation || []),
      ],
      beforeDelete: [
        async ({ id, req }) => {
          const doc = await req.payload.findByID({
            collection: "users",
            id,
            overrideAccess: true,
            req,
          });
          if (protectedRecord(doc))
            throw new APIError(
              "The system operator cannot be deleted through the CMS.",
              403,
            );
        },
        ...(collection.hooks?.beforeDelete || []),
      ],
    },
  };
}

/** Keep each collection's existing public read rules; block read-only accounts from writes. */
export function protectContent<T extends CollectionConfig | GlobalConfig>(
  config: T,
): T {
  const access = { ...config.access };
  for (const operation of ["create", "update", "delete"] as const) {
    const existing =
      (access as Record<string, Access>)[operation] ||
      (({ req }: { req: PayloadRequest }) => Boolean(req.user));
    (access as Record<string, Access>)[operation] = (args) =>
      actor(args.req)?.collection === "users" &&
      (operation === "delete"
        ? ["admin", "operator"]
        : ["editor", "admin", "operator"]
      ).includes(actor(args.req)?.role || "")
        ? existing(args)
        : false;
  }
  return {
    ...config,
    access,
    hooks: {
      ...config.hooks,
      beforeOperation: [
        ({ operation, req }: { operation: string; req: PayloadRequest }) => {
          if (operation === "restoreVersion" && !canManageUsers({ req }))
            throw new APIError(
              "An administrator is required to restore content versions.",
              403,
            );
        },
        ...(config.hooks?.beforeOperation || []),
      ],
    },
  } as T;
}
