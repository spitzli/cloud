import { ModuleError } from "@webdock/module-sdk";
import { moduleForRequest } from "./installation";
import { HTTPError } from "./plugins/body";
import { RegistryError } from "./modules/registry/service";
import { ZodError } from "zod";
import Elysia from "elysia";
import { randomUUID } from "node:crypto";
import healthRoutes from "./routes/public/healthz";
import type { ApiConfig } from "./config";
import { validateMounts, type Mount } from "./route-manifest";
export async function createApp(
  config: ApiConfig,
  dependencies: { ready?: () => Promise<void>; mounts?: Mount[] } = {},
) {
  const publicRoutes = new Elysia().group("/healthz", app => app.use(healthRoutes));
  const readiness = new Elysia().get("/readyz", async () => {
    try {
      if (!dependencies.ready) throw Error("No readiness dependency");
      await dependencies.ready();
      return { status: "ready" };
    } catch {
      return Response.json({ status: "unavailable" }, { status: 503 });
    }
  });
  const mounts: Mount[] = [
    { app: publicRoutes, host: config.apiOrigin, category: "public" },
    { app: readiness, host: config.apiOrigin, category: "public" },
    ...(dependencies.mounts || []),
  ];
  const origins = [config.apiOrigin, config.authOrigin, config.studioOrigin];
  validateMounts(mounts, origins);
  const app = new Elysia()
    .onRequest(({ request, set }) => {
      set.headers["x-request-id"] = randomUUID();
      set.headers["cache-control"] = "no-store";
      set.headers["x-content-type-options"] = "nosniff";
      if (!origins.includes(new URL(request.url).origin))
        return new Response(null, { status: 404 });
      const requestedModule = moduleForRequest(new URL(request.url).pathname);
      if (requestedModule && config.modules?.modules[requestedModule] === "disabled")
        return Response.json(
          { error: { code: "module_disabled", message: `${requestedModule} is disabled.` } },
          { status: 404 },
        );
      // Until the ingress transport is explicitly authenticated, forwarding headers
      // cannot redefine identity, host, scheme or rate-limit IP.
      if (
        [
          "forwarded",
          "x-forwarded-host",
          "x-forwarded-proto",
          "x-vercel-forwarded-for",
        ].some((name) => request.headers.has(name))
      )
        return new Response(null, { status: 400 });
    })
    .onError(({ code, error, set }) => {
      const status =
        error instanceof ModuleError ||
        error instanceof HTTPError ||
        error instanceof RegistryError
          ? error.status
          : error instanceof ZodError
            ? 400
            : code === "NOT_FOUND"
              ? 404
              : code === "VALIDATION" || code === "PARSE"
                ? 400
                : 500;
      set.status = status;
      return {
        error: {
          code:
            error instanceof ModuleError
              ? error.code
              : status === 500
                ? "internal_error"
                : "invalid_request",
          message:
            error instanceof ModuleError ||
            error instanceof HTTPError ||
            error instanceof RegistryError
              ? error.message
              : status === 500
                ? "Request failed."
                : "Request unavailable.",
          requestId: set.headers["x-request-id"],
        },
      };
    });
  for (const mount of mounts) {
    const guarded = new Elysia()
      .guard({
        beforeHandle: async ({ request }) => {
          if (
            ![mount.host, ...(mount.aliases ?? [])].includes(
              new URL(request.url).origin,
            )
          )
            return new Response(null, { status: 404 });
          if (
            ["tenant", "operator"].includes(mount.category) &&
            !["GET", "HEAD", "OPTIONS"].includes(request.method) &&
            ![mount.host, config.studioOrigin].includes(
              request.headers.get("origin") || "",
            )
          )
            return new Response(null, { status: 403 });
          return await mount.guard?.(request);
        },
      })
      .use(mount.app);
    app.use(guarded);
  }
  return app.compile();
}
