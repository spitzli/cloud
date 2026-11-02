import { validateDomainOwnership } from "./domain-ownership";
import { readAuthProtocolSelection, createAuthProtocolMounts } from "./protocols/auth-startup";
import { validateBrowserAuthConfiguration } from "./modules/auth/clerk-runtime";
import { normalizeIngress } from "./ingress";
import { createApp } from "./app";
import { readConfig } from "./config";
type ServerExtension = {
  mcp?: (config: ReturnType<typeof readConfig>) => Promise<import("./route-manifest").Mount[]>;
  mounts?: (config: ReturnType<typeof readConfig>) => Promise<import("./route-manifest").Mount[]>;
};
export async function startServer(extension: ServerExtension = {}) {
  const config = readConfig(process.env);
  const authSelection = readAuthProtocolSelection(process.env);
  validateDomainOwnership(config.domains);
  await validateBrowserAuthConfiguration();
  if (
    authSelection.oauthProvider === "better-auth" &&
    config.domains.some((domain) =>
      ["portal", "registry", "integrations", "mcp"].includes(domain),
    ) &&
    (process.env.WEBDOCK_AUTH_ISSUER !== config.authOrigin + "/api/auth" ||
      process.env.WEBDOCK_SSO_APP_ORIGIN !== config.studioOrigin ||
      process.env.WEBDOCK_SSO_CLIENT_ID !== process.env.WEBDOCK_STUDIO_CLIENT_ID)
  )
    throw Error(
      "Studio protocol configuration must match canonical origins and client",
    );
  const { databasePools: databases } = await import("./db/runtime");
  const {verifyPluginStartup}=await import('./modules/plugins/readiness');
  await verifyPluginStartup(databases.worker);
  let views:
    ReturnType<(typeof import("./cache-redis"))["connectViewCache"]> | undefined;
  if (process.env.VIEW_CACHE_URL) {
    try {
      views = (await import("./cache-redis")).connectViewCache(
        process.env.VIEW_CACHE_URL,
      );
    } catch {
      console.warn(JSON.stringify({ event: "view_cache_unavailable" }));
    }
  }
  await (await import("./module-preflight")).verifyModuleShutdown();
  const { configureViewCache } = await import("./cache");
  let cachePruneTimer: ReturnType<typeof setInterval> | undefined;
  try {
    const ready =
      (
        await databases.authentication.query(
          "SELECT to_regprocedure('webdock_cache.version(text[])') IS NOT NULL AND to_regprocedure('webdock_cache.prune()') IS NOT NULL AND to_regclass('webdock_cache.events') IS NOT NULL AS ready",
        )
      ).rows[0]?.ready === true;
    if (ready) {
      let pruning = false;
      cachePruneTimer = setInterval(async () => {
        if (pruning) return;
        pruning = true;
        try {
          await databases.authentication.query("SELECT webdock_cache.prune()");
        } catch {
          console.warn(JSON.stringify({ event: "view_cache_prune_unavailable" }));
        } finally {
          pruning = false;
        }
      }, 60_000);
      cachePruneTimer.unref();
      if (views && (!config.production || process.env.VIEW_CACHE_RELEASE)) {
        configureViewCache(views.cache, {
          environment: `${process.env.VIEW_CACHE_ENVIRONMENT || `${config.production ? "production" : "development"}:${config.apiOrigin}`}:${process.env.VIEW_CACHE_RELEASE || "development"}`,
          defaultTTLSeconds: Number(process.env.VIEW_CACHE_TTL_SECONDS || 60),
        });
      }
    } else if (views)
      console.warn(
        JSON.stringify({ event: "view_cache_revision_schema_unavailable" }),
      );
  } catch {
    console.warn(
      JSON.stringify({ event: "view_cache_configuration_unavailable" }),
    );
  }
  if (views && config.production && !process.env.VIEW_CACHE_RELEASE)
    console.warn(JSON.stringify({ event: "view_cache_release_unavailable" }));
  const mounts = await extension.mounts?.(config) ?? [];
  mounts.push(...await createAuthProtocolMounts(config, authSelection));
  if (config.domains.includes("portal"))
    mounts.push(...(await import("./protocols/portal")).portalProtocols(config));
  if (config.domains.includes("portal"))
    mounts.push(...(await import("./protocols/github")).githubProtocols(config));
  if (config.domains.includes("registry")) mounts.push((await import("./protocols/plugins")).pluginProtocol(config));
  if (config.domains.includes("registry")) mounts.push((await import("./protocols/plugin-installer")).pluginInstallerProtocol(config));
  if (config.domains.includes("registry"))
    mounts.push(
      await (await import("./protocols/registry")).registryProtocol(config),
    );
  if (config.domains.includes("mcp")) {
    if (process.env.WEBDOCK_MCP_RESOURCE !== config.studioOrigin + "/api/mcp") throw Error("MCP resource must preserve the canonical Studio audience");
    mounts.push(...await (extension.mcp ? extension.mcp(config) : Promise.resolve().then(async () => (await import("./protocols/mcp")).mcpProtocol(config))));
  }
  const ready = async () => {
    await verifyPluginStartup(databases.worker);
    if (config.production) {
      const registry = config.domains.includes("registry")
        ? (await (await import("./modules/registry/context")).registryService())
            .raw
        : undefined;
      await (
        await import("./db/readiness")
      ).verifyDatabaseReadiness(databases, config.domains, registry);
    } else await databases.authentication.query("SELECT 1");
  };
  await ready();
  const app = await createApp(config, { mounts, ready });
  const { handleConsoleOrigin } = await import("./console-origin");
  const ingressSecret = process.env.WEBDOCK_INGRESS_SECRET;
  const trustedProxies = (process.env.WEBDOCK_TRUSTED_PROXIES || "")
    .split(",")
    .filter(Boolean);
  if (
    config.production &&
    (!ingressSecret || ingressSecret.length < 32 || !trustedProxies.length)
  )
    throw Error("Verified ingress configuration is required");
  const server = Bun.serve({
    hostname: process.env.BIND_ADDRESS || "127.0.0.1",
    port: config.port,
    maxRequestBodySize: 4_000_000,
    fetch: (request) => {
      const normalized = normalizeIngress(
        request,
        server.requestIP(request)?.address,
        {
          origins: [
            config.apiOrigin,
            config.authOrigin,
            config.studioOrigin,
            ...(config.consoleOrigin ? [config.consoleOrigin] : []),
          ],
          trustedProxies,
          secret: ingressSecret,
          production: config.production,
        },
      );
      return normalized instanceof Response
        ? normalized
        : handleConsoleOrigin(normalized, config, (request) =>
            app.handle(request),
          );
    },
  });
  let stopping = false;
  async function stop() {
    if (stopping) return;
    stopping = true;
    if (cachePruneTimer) clearInterval(cachePruneTimer);
    const deadline = setTimeout(() => process.exit(1), 15_000);
    deadline.unref();
    try {
      await server.stop();
      views?.close();
      await databases.close();
      await (await import("./modules/registry/context")).closeRegistry();
    } finally {
      clearTimeout(deadline);
    }
  }
  process.once("SIGTERM", stop);
  process.once("SIGINT", stop);

}
