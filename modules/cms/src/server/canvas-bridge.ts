import { CMSBridgeError } from "./errors";
import {
  parseCanvasDocument,
  type CanvasPage,
  type CanvasList,
  type CanvasPageSummary,
} from "@spitzli/page-builder/model";
import type { createCMSDemoBridges } from "./demo-bridges";
import type { CMSPorts } from "./ports";
export type CanvasCommand =
  | { action: "create"; document: unknown }
  | { action: "save"; id: number; expectedRevision: number; document: unknown }
  | { action: "publish" | "unpublish"; id: number; expectedRevision: number }
  | { action: "homepage"; id: number | null; expectedRevision?: number };

const positive = (value: unknown): value is number =>
  Number.isSafeInteger(value) && Number(value) > 0;
export interface CMSCanvasBridgeDependencies {
  demoAccess: ReturnType<typeof createCMSDemoBridges>["demoAccess"];
  getStudioSession: CMSPorts["getStudioSession"];
  fetch: CMSPorts["fetch"];
}
export function createCMSCanvasBridge(
  dependencies: CMSCanvasBridgeDependencies,
) {
  const { demoAccess, getStudioSession, fetch } = dependencies;
  function page(value: unknown): CanvasPage {
    const p = value as CanvasPage;
    if (
      !p ||
      !positive(p.id) ||
      !positive(p.draftRevision) ||
      (p.publishedRevision !== null && !positive(p.publishedRevision)) ||
      typeof p.updatedAt !== "string" ||
      !Number.isFinite(Date.parse(p.updatedAt)) ||
      (p.publishedAt !== null &&
        (typeof p.publishedAt !== "string" ||
          !Number.isFinite(Date.parse(p.publishedAt))))
    )
      throw new CMSBridgeError("Ungültige Antwort des Builders.");
    return {
      id: p.id,
      draft: parseCanvasDocument(p.draft),
      draftRevision: p.draftRevision,
      published: p.published === null ? null : parseCanvasDocument(p.published),
      publishedRevision: p.publishedRevision,
      publishedAt: p.publishedAt,
      updatedAt: p.updatedAt,
    };
  }

  async function request(
    bindingID: string,
    id?: number,
    command?: CanvasCommand,
  ): Promise<unknown> {
    const { bridge, site } = await demoAccess(bindingID, !!command);
    if (!bridge.canvas)
      throw new CMSBridgeError(
        "Der Builder ist für diese Website noch nicht verfügbar.",
      );
    const session = await getStudioSession();
    if (!session) throw new CMSBridgeError("Bitte erneut anmelden.");
    const response = await fetch(
      `${bridge.origin}/api/webdock/canvas${id ? `?id=${id}` : ""}`,
      {
        method: command ? "POST" : "GET",
        cache: "no-store",
        redirect: "error",
        signal: AbortSignal.timeout(30000),
        headers: {
          Authorization: `Bearer ${bridge.secret}`,
          "Content-Type": "application/json",
          "X-Webdock-Role": site.role,
          "X-Webdock-Actor": session.user.id,
        },
        ...(command ? { body: JSON.stringify(command) } : {}),
      },
    );
    const reader = response.body?.getReader();
    if (!reader) throw new CMSBridgeError("Der Builder ist nicht erreichbar.");
    const chunks: Uint8Array[] = [];
    let size = 0;
    try {
      for (;;) {
        const part = await reader.read();
        if (part.done) break;
        size += part.value.byteLength;
        if (size > 2_000_000)
          throw new CMSBridgeError("Die Builder-Antwort ist zu groß.");
        chunks.push(part.value);
      }
    } finally {
      void reader.cancel().catch(() => {});
    }
    const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    if (!response.ok) {
      if (
        [400, 403, 404, 409].includes(response.status) &&
        typeof body.error === "string"
      )
        throw new CMSBridgeError(body.error.slice(0, 400));
      throw new CMSBridgeError(
        "Der Builder ist gerade nicht erreichbar. Bitte neu laden und den gespeicherten Stand prüfen.",
      );
    }
    return body;
  }

  async function canvasList(bindingID: string): Promise<CanvasList> {
    const result = (await request(bindingID)) as CanvasList;
    if (
      !result ||
      !Array.isArray(result.pages) ||
      result.pages.length > 100 ||
      (result.homePageID !== null && !positive(result.homePageID)) ||
      !["/original", "/old", "/shop"].includes(result.nativePath)
    )
      throw new CMSBridgeError("Ungültige Seitenübersicht.");
    const pages = result.pages.map((p: CanvasPageSummary) => {
      if (
        !positive(p.id) ||
        typeof p.title !== "string" ||
        p.title.length > 120 ||
        typeof p.slug !== "string" ||
        !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(p.slug) ||
        !positive(p.draftRevision) ||
        (p.publishedRevision !== null && !positive(p.publishedRevision)) ||
        (p.publishedSlug !== null &&
          (typeof p.publishedSlug !== "string" ||
            !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(p.publishedSlug))) ||
        typeof p.updatedAt !== "string"
      )
        throw new CMSBridgeError("Ungültiger Seiteneintrag.");
      return {
        id: p.id,
        title: p.title,
        slug: p.slug,
        draftRevision: p.draftRevision,
        publishedRevision: p.publishedRevision,
        publishedSlug: p.publishedSlug,
        updatedAt: p.updatedAt,
      };
    });
    return {
      pages,
      homePageID: result.homePageID,
      nativePath: result.nativePath,
    };
  }

  async function canvasPage(
    bindingID: string,
    id: number,
  ): Promise<CanvasPage> {
    if (!positive(id)) throw new CMSBridgeError("Ungültige Seite.");
    return page(((await request(bindingID, id)) as { page: unknown }).page);
  }

  async function writeCanvas(
    bindingID: string,
    input: unknown,
  ): Promise<{ page?: CanvasPage; homePageID?: number | null }> {
    if (!input || typeof input !== "object" || Array.isArray(input))
      throw new CMSBridgeError("Ungültige Builder-Aktion.");
    const value = input as Record<string, unknown>;
    let command: CanvasCommand;
    if (value.action === "create")
      command = {
        action: "create",
        document: parseCanvasDocument(value.document),
      };
    else if (
      value.action === "save" &&
      positive(value.id) &&
      positive(value.expectedRevision)
    )
      command = {
        action: "save",
        id: value.id,
        expectedRevision: value.expectedRevision,
        document: parseCanvasDocument(value.document),
      };
    else if (
      ["publish", "unpublish"].includes(String(value.action)) &&
      positive(value.id) &&
      positive(value.expectedRevision)
    )
      command = {
        action: value.action as "publish" | "unpublish",
        id: value.id,
        expectedRevision: value.expectedRevision,
      };
    else if (
      value.action === "homepage" &&
      (value.id === null || positive(value.id)) &&
      (value.id === null || positive(value.expectedRevision))
    )
      command = {
        action: "homepage",
        id: value.id as number | null,
        ...(value.id !== null
          ? { expectedRevision: value.expectedRevision as number }
          : {}),
      };
    else throw new CMSBridgeError("Ungültige Builder-Aktion oder Version.");
    const result = (await request(bindingID, undefined, command)) as {
      page?: unknown;
      homePageID?: unknown;
    };
    if (command.action === "homepage") {
      if (result.homePageID !== null && !positive(result.homePageID))
        throw new CMSBridgeError(
          "Die Änderung wurde verarbeitet. Bitte den aktuellen Stand neu laden.",
        );
      return { homePageID: result.homePageID as number | null };
    }
    return { page: page(result.page) };
  }
  return { canvasList, canvasPage, writeCanvas };
}
