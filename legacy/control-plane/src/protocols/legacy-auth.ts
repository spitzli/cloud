// Explicit legacy protocol mounts preserve public paths while their services move.
import Elysia from "elysia";
import type { ApiConfig } from "../config";
import type { Mount, RouteCategory } from "../route-manifest";
type Handler = (
  request: Request,
  context: { params: Promise<Record<string, string | string[]>> },
) => Response | Promise<Response>;
export async function legacyAuthProtocols(config: ApiConfig): Promise<Mount[]> {
  const mounts: Mount[] = [];
  if (config.domains.includes("databases") && config.modules?.modules.databases !== "disabled") {
    const handlers = await import("./legacy-auth/api/databases/bridge/route");
    const app = new Elysia();
    app.route(
      "POST",
      "/api/databases/bridge",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({ app, host: config.authOrigin, category: "delegated" });
  }
  if (config.domains.includes("databases") && config.modules?.modules.databases !== "disabled") {
    const handlers = await import("./legacy-auth/api/databases/gateway/route");
    const app = new Elysia();
    app.route(
      "POST",
      "/api/databases/gateway",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({ app, host: config.authOrigin, category: "agent" });
  }
  // Shared authenticated agent transport also reconciles Mail and database resources.
  if (config.domains.includes("hosting")) {
    const handlers =
      await import("./legacy-auth/api/hosting/agent/[...path]/route");
    const app = new Elysia();
    app.route(
      "POST",
      "/api/hosting/agent/*",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          {
            params: Promise.resolve(
              Object.assign({}, params, {
                path: (params["*"] || "").split("/").filter(Boolean),
              }),
            ),
          },
        ]),
      { parse: "none" },
    );
    mounts.push({ app, host: config.authOrigin, category: "agent" });
  }
  if (config.domains.includes("hosting") && config.modules?.modules.hosting !== "disabled") {
    const handlers = await import("./legacy-auth/api/hosting/bridge/route");
    const app = new Elysia();
    app.route(
      "POST",
      "/api/hosting/bridge",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({ app, host: config.authOrigin, category: "delegated" });
  }
  if (config.domains.includes("hosting") && config.modules?.modules.hosting !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/hosting/git/webhook/route");
    const app = new Elysia();
    app.route(
      "POST",
      "/api/hosting/git/webhook",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({ app, host: config.authOrigin, category: "signed_webhook" });
  }
  if (config.domains.includes("hosting") && config.modules?.modules.hosting !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/hosting/git/worker/[...path]/route");
    const app = new Elysia();
    app.route(
      "GET",
      "/api/hosting/git/worker/*",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          {
            params: Promise.resolve(
              Object.assign({}, params, {
                path: (params["*"] || "").split("/").filter(Boolean),
              }),
            ),
          },
        ]),
      { parse: "none" },
    );
    app.route(
      "POST",
      "/api/hosting/git/worker/*",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          {
            params: Promise.resolve(
              Object.assign({}, params, {
                path: (params["*"] || "").split("/").filter(Boolean),
              }),
            ),
          },
        ]),
      { parse: "none" },
    );
    mounts.push({ app, host: config.authOrigin, category: "agent" });
  }
  if (config.domains.includes("lifecycle")) {
    const handlers = await import("./legacy-auth/api/lifecycle/route");
    const app = new Elysia();
    app.route(
      "POST",
      "/api/lifecycle",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({ app, host: config.authOrigin, category: "delegated" });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers = await import("./legacy-auth/api/mail/gateway/route");
    const app = new Elysia();
    app.route(
      "POST",
      "/api/mail/gateway",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({ app, host: config.authOrigin, category: "agent" });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/mail/provider-events/[sourceID]/route");
    const app = new Elysia();
    app.route(
      "POST",
      "/api/mail/provider-events/:sourceID",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({ app, host: config.authOrigin, category: "signed_webhook" });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers = await import("./legacy-auth/api/mail/v1/analytics/route");
    const app = new Elysia();
    app.route(
      "GET",
      "/api/mail/v1/analytics",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/mail/v1/api-keys/[id]/route");
    const app = new Elysia();
    app.route(
      "DELETE",
      "/api/mail/v1/api-keys/:id",
      ({ request, params }) =>
        Reflect.apply(handlers.DELETE, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers = await import("./legacy-auth/api/mail/v1/api-keys/route");
    const app = new Elysia();
    app.route(
      "GET",
      "/api/mail/v1/api-keys",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    app.route(
      "POST",
      "/api/mail/v1/api-keys",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/mail/v1/batches/[id]/route");
    const app = new Elysia();
    app.route(
      "GET",
      "/api/mail/v1/batches/:id",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/mail/v1/domains/[id]/route");
    const app = new Elysia();
    app.route(
      "DELETE",
      "/api/mail/v1/domains/:id",
      ({ request, params }) =>
        Reflect.apply(handlers.DELETE, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    app.route(
      "GET",
      "/api/mail/v1/domains/:id",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/mail/v1/domains/[id]/verify/route");
    const app = new Elysia();
    app.route(
      "POST",
      "/api/mail/v1/domains/:id/verify",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers = await import("./legacy-auth/api/mail/v1/domains/route");
    const app = new Elysia();
    app.route(
      "GET",
      "/api/mail/v1/domains",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    app.route(
      "POST",
      "/api/mail/v1/domains",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/mail/v1/emails/[id]/cancel/route");
    const app = new Elysia();
    app.route(
      "POST",
      "/api/mail/v1/emails/:id/cancel",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/mail/v1/emails/[id]/route");
    const app = new Elysia();
    app.route(
      "GET",
      "/api/mail/v1/emails/:id",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    app.route(
      "PATCH",
      "/api/mail/v1/emails/:id",
      ({ request, params }) =>
        Reflect.apply(handlers.PATCH, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/mail/v1/emails/batch/route");
    const app = new Elysia();
    app.route(
      "POST",
      "/api/mail/v1/emails/batch",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers = await import("./legacy-auth/api/mail/v1/emails/route");
    const app = new Elysia();
    app.route(
      "GET",
      "/api/mail/v1/emails",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    app.route(
      "POST",
      "/api/mail/v1/emails",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers = await import("./legacy-auth/api/mail/v1/logs/[id]/route");
    const app = new Elysia();
    app.route(
      "GET",
      "/api/mail/v1/logs/:id",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers = await import("./legacy-auth/api/mail/v1/logs/route");
    const app = new Elysia();
    app.route(
      "GET",
      "/api/mail/v1/logs",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/mail/v1/openapi.json/route");
    const app = new Elysia();
    app.route(
      "GET",
      "/api/mail/v1/openapi.json",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "public",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/mail/v1/suppressions/[email]/route");
    const app = new Elysia();
    app.route(
      "DELETE",
      "/api/mail/v1/suppressions/:email",
      ({ request, params }) =>
        Reflect.apply(handlers.DELETE, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/mail/v1/suppressions/route");
    const app = new Elysia();
    app.route(
      "GET",
      "/api/mail/v1/suppressions",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    app.route(
      "POST",
      "/api/mail/v1/suppressions",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/mail/v1/webhooks/[id]/deliveries/[deliveryID]/replay/route");
    const app = new Elysia();
    app.route(
      "POST",
      "/api/mail/v1/webhooks/:id/deliveries/:deliveryID/replay",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/mail/v1/webhooks/[id]/deliveries/route");
    const app = new Elysia();
    app.route(
      "GET",
      "/api/mail/v1/webhooks/:id/deliveries",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/mail/v1/webhooks/[id]/rotate/route");
    const app = new Elysia();
    app.route(
      "POST",
      "/api/mail/v1/webhooks/:id/rotate",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers =
      await import("./legacy-auth/api/mail/v1/webhooks/[id]/route");
    const app = new Elysia();
    app.route(
      "DELETE",
      "/api/mail/v1/webhooks/:id",
      ({ request, params }) =>
        Reflect.apply(handlers.DELETE, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    app.route(
      "GET",
      "/api/mail/v1/webhooks/:id",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    app.route(
      "PATCH",
      "/api/mail/v1/webhooks/:id",
      ({ request, params }) =>
        Reflect.apply(handlers.PATCH, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers = await import("./legacy-auth/api/mail/v1/webhooks/route");
    const app = new Elysia();
    app.route(
      "GET",
      "/api/mail/v1/webhooks",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    app.route(
      "POST",
      "/api/mail/v1/webhooks",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({
      app,
      host: config.apiOrigin,
      aliases: [config.authOrigin],
      category: "mail_key",
    });
  }
  if (config.domains.includes("auth")) {
    const handlers = await import("./legacy-auth/api/mcp/introspect/route");
    const app = new Elysia();
    app.route(
      "POST",
      "/api/mcp/introspect",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({ app, host: config.authOrigin, category: "mcp" });
  }
  if (config.domains.includes("portal")) {
    const handlers = await import("./legacy-auth/api/studio/route");
    const app = new Elysia();
    app.route(
      "POST",
      "/api/studio",
      ({ request, params }) =>
        Reflect.apply(handlers.POST, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({ app, host: config.authOrigin, category: "delegated" });
  }
  if (config.domains.includes("mail") && config.modules?.modules.mail !== "disabled") {
    const handlers = await import("./legacy-auth/mail/docs/route");
    const app = new Elysia();
    app.route(
      "GET",
      "/mail/docs",
      ({ request, params }) =>
        Reflect.apply(handlers.GET, undefined, [
          request,
          { params: Promise.resolve(Object.assign({}, params)) },
        ]),
      { parse: "none" },
    );
    mounts.push({ app, host: config.authOrigin, category: "public" });
  }
  return mounts;
}
