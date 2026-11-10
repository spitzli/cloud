import { msgid } from "@spitzli/i18n";
/* eslint-disable @typescript-eslint/no-explicit-any -- Explicit module and field allowlists constrain dynamic CMS schemas; native validation remains authoritative. */
import { APIError, createLocalReq, type Payload } from "payload";
import { cmsFields, defaultData, editableData, CMSInputError } from "./schema";
import type { CMSOptions, CMSModule } from "./types";
const roles = ["reader", "editor", "admin", "operator"];
export async function cmsUser(payload: Payload, headers: Headers) {
  const { user: authenticated } = await payload.auth({ headers });
  const user = authenticated as
    | (NonNullable<typeof authenticated> & {
        role: string;
        name?: string;
        disabled?: boolean;
      })
    | null;
  return user &&
    user.collection === "users" &&
    !user.disabled &&
    roles.includes(String(user.role))
    ? user
    : null;
}
const json = (data: unknown, status = 200) =>
  Response.json(data, {
    status,
    headers: {
      "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
    },
  });
const recordID = (value: unknown) => {
  if (
    typeof value !== "string" ||
    !value ||
    value.length > 128 ||
    !/^[-\w]+$/.test(value)
  )
    throw new CMSInputError(msgid("Invalid record."));
  return value;
};
const boundedPage = (value: string | null) => {
  if (value === null) return 1;
  if (!/^\d+$/.test(value) || Number(value) < 1 || Number(value) > 10000)
    throw new CMSInputError(msgid("Invalid page."));
  return Number(value);
};
const moduleConfig = (p: Payload, m: CMSModule) =>
  m.kind === "collection"
    ? (p.collections as any)[m.slug]?.config
    : p.config.globals.find((g) => g.slug === m.slug);
export type CMSRequestAdapter = {
  /** Set only by verified server middleware, never from a browser request body. */
  trustedUser?: NonNullable<Awaited<ReturnType<typeof cmsUser>>>;
  uploads?: {
    maxFileBytes: number;
    maxBodyBytes: number;
    metadataBytes?: number;
    /** Existing media stays immutable when local files cannot roll back with the DB. */
    allowFileReplacement?: boolean;
    /** Only for reviewed upload collections with native trash enabled. */
    removeToTrash?: boolean;
    validateFile?: (file: {
      data: Buffer;
      name: string;
      mimetype: string;
      size: number;
    }) => Promise<{
      data: Buffer;
      name: string;
      mimetype: string;
      size: number;
    }>;
  };
};
async function bodyOf(request: Request, adapter: CMSRequestAdapter) {
  const limit = adapter.uploads?.maxBodyBytes ?? 10 * 1024 * 1024,
    reader = request.body?.getReader(),
    chunks: Uint8Array[] = [];
  let size = 0;
  if (reader)
    try {
      while (true) {
        const item = await reader.read();
        if (item.done) break;
        size += item.value.length;
        if (size > limit) {
          await reader.cancel();
          throw new CMSInputError(msgid("The upload is too large."), 413);
        }
        chunks.push(item.value);
      }
    } finally {
      reader.releaseLock();
    }
  const bytes = Buffer.concat(chunks),
    type = request.headers.get("content-type") || "";
  if (type.startsWith("multipart/form-data")) {
    const form = await new Request(request.url, {
      method: "POST",
      headers: { "content-type": type },
      body: bytes,
    }).formData();
    let input;
    try {
      const metadata = String(form.get("data"));
      if (
        Buffer.byteLength(metadata) >
        (adapter.uploads?.metadataBytes ?? 1024 * 1024)
      )
        throw new CMSInputError(msgid("Content is too large."), 413);
      input = JSON.parse(metadata);
    } catch {
      throw new CMSInputError(msgid("Invalid form data."));
    }
    const file = form.get("file");
    if (file instanceof File && file.size)
      input.file = {
        data: Buffer.from(await file.arrayBuffer()),
        name: file.name,
        mimetype: file.type,
        size: file.size,
      };
    return input;
  }
  if (!type.startsWith("application/json"))
    throw new CMSInputError(msgid("Use JSON or a file upload."), 415);
  if (bytes.length > 1024 * 1024)
    throw new CMSInputError(msgid("Content is too large."), 413);
  try {
    return JSON.parse(bytes.toString("utf8"));
  } catch {
    throw new CMSInputError(msgid("Invalid JSON."));
  }
}
export async function handleCMSRequest(
  request: Request,
  options: CMSOptions,
  getPayload: () => Promise<Payload>,
  adapter: CMSRequestAdapter = {},
) {
  let p: Payload | undefined,
    transaction: string | number | null = null;
  try {
    if (!["GET", "POST", "DELETE"].includes(request.method))
      return new Response(null, {
        status: 405,
        headers: { Allow: "GET, POST, DELETE" },
      });
    if (request.method !== "GET") {
      const origin = request.headers.get("origin");
      if (
        !origin ||
        ![
          new URL(options.siteURL).origin,
          new URL(request.url).origin,
        ].includes(origin)
      )
        throw new CMSInputError(msgid("Reload the CMS and try again."), 403);
    }
    p = await getPayload();
    const user = adapter.trustedUser ?? (await cmsUser(p, request.headers));
    if (!user) throw new CMSInputError(msgid("Sign in to open the CMS."), 401);
    const canWrite = user.role !== "reader",
      canDelete = ["admin", "operator"].includes(String(user.role));
    if (request.method !== "GET" && !canWrite)
      throw new CMSInputError(
        msgid("Your role can view content but cannot change it."),
        403,
      );
    if (request.method === "DELETE" && !canDelete)
      throw new CMSInputError(
        msgid("An administrator is required to delete content."),
        403,
      );
    const url = new URL(request.url),
      input =
        request.method === "GET"
          ? Object.fromEntries(url.searchParams)
          : await bodyOf(request, adapter);
    if (!input || typeof input !== "object" || Array.isArray(input))
      throw new CMSInputError(msgid("Invalid request."));
    const locale = input.locale || options.defaultLocale;
    if (
      locale &&
      !(options.locales || [options.defaultLocale]).includes(locale)
    )
      throw new CMSInputError(msgid("Choose a supported language."));
    if (!input.module && request.method === "GET")
      return json({
        siteName: options.siteName,
        siteURL: options.siteURL,
        user: { name: user.name || user.email, role: user.role },
        canWrite,
        canDelete,
        locales: options.locales || [],
        defaultLocale: options.defaultLocale,
        modules: options.modules.map((m) => {
          const c = moduleConfig(p!, m);
          return {
            ...m,
            drafts: Boolean(c?.versions && c.versions.drafts),
            versions: Boolean(c?.versions),
            upload: Boolean(c?.upload),
          };
        }),
      });
    const module = options.modules.find((m) => m.slug === input.module);
    if (
      !module ||
      module.slug === "users" ||
      module.slug.startsWith("payload-")
    )
      throw new CMSInputError(
        msgid("This content area is not available."),
        404,
      );
    const config = moduleConfig(p, module);
    if (!config)
      throw new CMSInputError(
        msgid("Content configuration is unavailable."),
        404,
      );
    const fields = cmsFields(config.fields),
      drafts = Boolean(config.versions && config.versions.drafts);
    const common: any = {
      user,
      overrideAccess: false,
      depth: 0,
      ...(locale ? { locale, fallbackLocale: false } : {}),
    };
    const descriptor = {
      ...module,
      fields,
      drafts,
      versions: Boolean(config.versions),
      upload: Boolean(config.upload),
      deleteMode:
        config.upload && adapter.uploads?.removeToTrash ? "trash" : "delete",
      canReplaceFile:
        Boolean(config.upload) &&
        adapter.uploads?.allowFileReplacement !== false,
      localizedStatus: Boolean(config.versions?.drafts?.localizeStatus),
      canWrite,
      canCreate: canWrite && module.create !== false,
      canDelete,
    };
    const read = async (req?: any) =>
      module.kind === "global"
        ? p!.findGlobal({
            slug: module.slug,
            ...common,
            req,
            draft: drafts,
          } as never)
        : p!.findByID({
            collection: module.slug,
            id: recordID(input.id),
            ...common,
            req,
            draft: drafts,
          } as never);
    if (request.method === "GET") {
      if (input.operation === "versions") {
        if (!config.versions)
          throw new CMSInputError(
            msgid("Version history is not enabled for this content."),
          );
        const result =
          module.kind === "global"
            ? await p.findGlobalVersions({
                slug: module.slug,
                ...common,
                limit: 20,
                page: boundedPage(input.page ?? null),
                sort: "-updatedAt",
              } as never)
            : await p.findVersions({
                collection: module.slug,
                ...common,
                where: { parent: { equals: recordID(input.id) } },
                limit: 20,
                page: boundedPage(input.page ?? null),
                sort: "-updatedAt",
              } as never);
        return json({
          docs: result.docs.map((v: any) => ({
            id: v.id,
            createdAt: v.createdAt,
            updatedAt: v.updatedAt,
            status: v.version?._status,
          })),
          totalPages: result.totalPages,
          page: result.page,
        });
      }
      if (input.id === "new")
        return json({ ...descriptor, doc: defaultData(fields) });
      if (module.kind === "global" || input.id) {
        const doc = await read();
        const published =
          drafts && input.id !== "new"
            ? module.kind === "global"
              ? await p.findGlobal({
                  slug: module.slug,
                  ...common,
                  draft: false,
                } as never)
              : await p.findByID({
                  collection: module.slug,
                  id: recordID(input.id),
                  ...common,
                  draft: false,
                } as never)
            : null;
        return json({
          ...descriptor,
          doc,
          published: (published as any)?._status === "published",
        });
      }
      const search =
        typeof input.search === "string" ? input.search.slice(0, 160) : "";
      const title = module.titleField || config.admin?.useAsTitle || "title";
      const result = await p.find({
        collection: module.slug,
        ...common,
        limit: 20,
        page: boundedPage(input.page ?? null),
        sort: config.defaultSort || "-updatedAt",
        draft: drafts,
        ...(search
          ? { where: { [module.searchField || title]: { contains: search } } }
          : {}),
      } as never);
      return json({ ...descriptor, ...result, titleField: title });
    }
    const mode = request.method === "DELETE" ? "delete" : input.mode || "save";
    if (
      !["save", "draft", "publish", "unpublish", "restore", "delete"].includes(
        mode,
      )
    )
      throw new CMSInputError(msgid("Unknown content action."));
    if (mode === "delete" && !canDelete)
      throw new CMSInputError(
        msgid("An administrator is required to delete content."),
        403,
      );
    if (["draft", "publish", "unpublish"].includes(mode) && !drafts)
      throw new CMSInputError(msgid("This content does not support drafts."));
    if (mode === "restore" && (!canDelete || !config.versions))
      throw new CMSInputError(
        msgid("An administrator is required to restore versions."),
        403,
      );
    if (mode === "delete" && module.kind === "global")
      throw new CMSInputError(msgid("Website settings cannot be deleted."));
    const isNew =
      module.kind === "collection" && (!input.id || input.id === "new");
    if (isNew && module.create === false)
      throw new CMSInputError(
        msgid("New records are created by the website in this content area."),
        403,
      );
    if (isNew && ["delete", "restore", "unpublish"].includes(mode))
      throw new CMSInputError(msgid("Select an existing record."));
    if (input.file && !isNew && adapter.uploads?.allowFileReplacement === false)
      throw new CMSInputError(
        msgid(
          "Existing images cannot be replaced. Upload a new image in Media, then select it in your content.",
        ),
        409,
      );
    transaction = await p.db.beginTransaction({
      isolationLevel: "serializable",
    });
    if (transaction === null)
      throw new CMSInputError(
        msgid("Content writes require transaction support."),
        503,
      );
    const req = await createLocalReq(
      { user, ...(locale ? { locale, fallbackLocale: false as const } : {}) },
      p,
    );
    req.transactionID = transaction;
    req.headers = new Headers(request.headers);
    if (!isNew) {
      const current: any = await read(req);
      if (
        current.updatedAt
          ? typeof input.updatedAt !== "string" ||
            input.updatedAt !== current.updatedAt
          : input.updatedAt !== null
      )
        throw new CMSInputError(
          msgid(
            "This content changed since you opened it. Reload before saving.",
          ),
          409,
        );
    }
    let result;
    if (mode === "delete") {
      if (descriptor.deleteMode === "trash") {
        if (!config.trash)
          throw new CMSInputError(
            msgid(
              "Media removal is unavailable until the retention migration has run.",
            ),
            503,
          );
        // Payload's hard delete unlinks files before DB commit. Native trash updates
        // metadata only; the reviewed collection also enforces delete roles itself.
        result = await p.update({
          collection: module.slug,
          id: recordID(input.id),
          data: { deletedAt: new Date().toISOString() },
          ...common,
          req,
        } as never);
      } else
        result = await p.delete({
          collection: module.slug,
          id: recordID(input.id),
          ...common,
          req,
        } as never);
    } else if (mode === "restore") {
      const versionID = recordID(input.versionID);
      if (module.kind === "collection") {
        const version: any = await p.findVersionByID({
          collection: module.slug,
          id: versionID,
          ...common,
          req,
        } as never);
        if (
          String(
            typeof version.parent === "object"
              ? version.parent.id
              : version.parent,
          ) !== String(input.id)
        )
          throw new CMSInputError(
            msgid("This version belongs to a different record."),
          );
        result = await p.restoreVersion({
          collection: module.slug,
          id: versionID,
          ...common,
          req,
        } as never);
      } else
        result = await p.restoreGlobalVersion({
          slug: module.slug,
          id: versionID,
          ...common,
          req,
        } as never);
    } else {
      // Payload's canary unpublish path copies other locales' pending statuses into
      // the live record. Keep its authorized operation/version, then preserve live locales.
      const liveBeforeUnpublish: any =
        mode === "unpublish" && descriptor.localizedStatus
          ? module.kind === "collection"
            ? await p.db.findOne({
                collection: module.slug,
                locale: "all",
                req,
                where: { id: { equals: recordID(input.id) } },
              } as never)
            : await p.db.findGlobal({
                slug: module.slug,
                locale: "all",
                req,
              } as never)
          : null;
      const data = mode === "unpublish" ? {} : editableData(fields, input.data);
      if (drafts) data._status = mode === "publish" ? "published" : "draft";
      let file = input.file;
      if (file) {
        if (
          !config.upload ||
          !Buffer.isBuffer(file.data) ||
          file.size >
            Math.min(
              adapter.uploads?.maxFileBytes ?? 8 * 1024 * 1024,
              p.config.upload?.limits?.fileSize || 8 * 1024 * 1024,
            )
        )
          throw new CMSInputError(
            msgid("This file is not allowed or is too large."),
            413,
          );
      }
      if (file && adapter.uploads?.validateFile)
        file = await adapter.uploads.validateFile(file);
      result =
        module.kind === "global"
          ? await p.updateGlobal({
              slug: module.slug,
              data,
              ...common,
              req,
              draft: !["publish", "unpublish"].includes(mode) && drafts,
            } as never)
          : isNew
            ? await p.create({
                collection: module.slug,
                data,
                ...common,
                req,
                draft: !["publish", "unpublish"].includes(mode) && drafts,
                ...(file ? { file } : {}),
              } as never)
            : await p.update({
                collection: module.slug,
                id: recordID(input.id),
                data,
                ...common,
                req,
                draft: !["publish", "unpublish"].includes(mode) && drafts,
                ...(file ? { file } : {}),
              } as never);
      if (liveBeforeUnpublish) {
        const publicationLocale = locale || req.locale;
        const liveAfterUnpublish: any =
          module.kind === "collection"
            ? await p.db.findOne({
                collection: module.slug,
                locale: "all",
                req,
                where: { id: { equals: recordID(input.id) } },
              } as never)
            : await p.db.findGlobal({
                slug: module.slug,
                locale: "all",
                req,
              } as never);
        const status = liveAfterUnpublish?._status?.[publicationLocale];
        if (status !== "draft")
          throw new CMSInputError(
            msgid("Your role cannot unpublish this content."),
            403,
          );
        const liveData = {
          ...liveBeforeUnpublish,
          _status: {
            ...liveBeforeUnpublish._status,
            [publicationLocale]: status,
          },
          updatedAt: (result as any).updatedAt,
        };
        if (module.kind === "collection")
          await p.db.updateOne({
            collection: module.slug,
            id: recordID(input.id),
            locale: "all",
            data: liveData,
            req,
          } as never);
        else
          await p.db.updateGlobal({
            slug: module.slug,
            data: liveData,
            req,
          } as never);
      }
    }
    await p.db.commitTransaction(transaction);
    transaction = null;
    return json(
      {
        doc: result,
        message:
          mode === "delete"
            ? descriptor.deleteMode === "trash"
              ? msgid(
                  "Image removed. Its file is retained until an operator purges it.",
                )
              : msgid("Content deleted.")
            : mode === "draft"
              ? msgid("Draft saved.")
              : mode === "unpublish"
                ? msgid("Unpublished.")
                : mode === "publish"
                  ? msgid("Published.")
                  : msgid("Changes saved."),
      },
      isNew ? 201 : 200,
    );
  } catch (error) {
    if (p && transaction !== null)
      await p.db.rollbackTransaction(transaction).catch(() => {});
    if (error instanceof CMSInputError)
      return json({ error: error.message }, error.status);
    if (error instanceof APIError && error.isPublic && error.status < 500)
      return json(
        {
          error: error.message,
          errors: "data" in error ? (error as any).data?.errors : undefined,
        },
        error.status,
      );
    const code = (error as any)?.code || (error as any)?.cause?.code;
    if (code === "40001")
      return json(
        {
          error: msgid(
            "Another edit was saved at the same time. Reload before trying again.",
          ),
        },
        409,
      );
    return json(
      {
        error: msgid(
          "The CMS could not complete this request. Your changes were not confirmed.",
        ),
      },
      500,
    );
  }
}
