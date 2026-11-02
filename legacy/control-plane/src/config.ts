import { publicInstallation } from "./installation";
import type { ModuleView } from "@webdock/module-sdk";
export type ApiConfig = {
  production: boolean;
  modules?: ModuleView;
  apiOrigin: string;
  authOrigin: string;
  studioOrigin: string;
  consoleOrigin?: string;
  port: number;
  domains: string[];
};
function origin(raw: string): string {
  const url = new URL(raw);
  if (
    url.username ||
    url.password ||
    url.pathname !== "/" ||
    url.search ||
    url.hash ||
    (url.protocol !== "https:" &&
      !(
        url.protocol === "http:" &&
        ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname)
      ))
  )
    throw Error("Invalid canonical origin");
  return url.origin;
}
export function readConfig(env: Record<string, string | undefined>): ApiConfig {
  const production = env.NODE_ENV === "production";
  const required = (key: string, fallback: string) => {
    if (production && !env[key]) throw Error(`Missing ${key}`);
    return origin(env[key] || fallback);
  };
  const apiOrigin = required("WEBDOCK_API_ORIGIN", "http://localhost:3130");
  const authOrigin = required("BETTER_AUTH_URL", "http://localhost:3125");
  const studioOrigin = required(
    "WEBDOCK_STUDIO_ORIGIN",
    "http://localhost:3120",
  );
  const consoleOrigin = env.WEBDOCK_CONSOLE_ORIGIN
    ? origin(env.WEBDOCK_CONSOLE_ORIGIN)
    : undefined;
  if (
    consoleOrigin &&
    ([apiOrigin, authOrigin, studioOrigin].includes(consoleOrigin) ||
      (production && !consoleOrigin.startsWith("https://")))
  )
    throw Error("Console origin must be distinct and secure");
  if (
    production &&
    [apiOrigin, authOrigin, studioOrigin].some(
      (url) => !url.startsWith("https://"),
    )
  )
    throw Error("Production requires HTTPS");
  if (new Set([apiOrigin, authOrigin, studioOrigin]).size !== 3)
    throw Error("Canonical origins must be distinct");
  const port = Number(env.PORT || 3130);
  if (!Number.isInteger(port) || port < 1 || port > 65535)
    throw Error("Invalid port");
  const domains = (env.WEBDOCK_API_DOMAINS ?? "auth,portal,registry").split(",").filter(Boolean);
  if (
    domains.some(
      (domain) =>
        ![
          "auth",
          "mail",
          "hosting",
          "databases",
          "portal",
          "lifecycle",
          "registry",
          "integrations",
          "mcp",
        ].includes(domain),
    )
  )
    throw Error("Unknown API domain");
  return {
    modules: publicInstallation(env),
    production,
    apiOrigin,
    authOrigin,
    studioOrigin,
    consoleOrigin,
    port,
    domains,
  };
}
