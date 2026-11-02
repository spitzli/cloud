type StudioEnvironment = Partial<
  Pick<
    NodeJS.ProcessEnv,
    "NODE_ENV" | "STUDIO_UI_ENABLED" | "WEBDOCK_STUDIO_ORIGIN" | "WEBDOCK_CONSOLE_ORIGIN"
  >
>;

export function studioURL(
  path = "/",
  env: StudioEnvironment = process.env,
): string {
  const origin = new URL(
    env.WEBDOCK_CONSOLE_ORIGIN || env.WEBDOCK_STUDIO_ORIGIN || "https://studio.webdock.dev",
  );
  const localHTTP =
    env.NODE_ENV !== "production" &&
    origin.protocol === "http:" &&
    ["localhost", "127.0.0.1", "[::1]"].includes(origin.hostname);
  if (
    origin.username ||
    origin.password ||
    origin.pathname !== "/" ||
    origin.search ||
    origin.hash ||
    (origin.protocol !== "https:" && !localHTTP)
  ) {
    throw new Error("WEBDOCK_STUDIO_ORIGIN must be a secure canonical origin.");
  }
  if (!path.startsWith("/") || path.startsWith("//") || path.includes("\\"))
    throw new Error("A local Studio path is required.");
  return new URL(path, origin).href;
}

export function businessURL(
  path: string,
  env: StudioEnvironment = process.env,
): string {
  return env.STUDIO_UI_ENABLED === "true" ? studioURL(path, env) : path;
}

export function legacyStudioRedirect(
  url: URL,
  method: string,
  env: StudioEnvironment = process.env,
): string | null {
  if (env.STUDIO_UI_ENABLED !== "true" || !["GET", "HEAD"].includes(method))
    return null;
  if (
    !/^\/(?:admin(?:\/.*)?|tenants(?:\/.*)?|offers\/.+|people\/?|sites\/?)$/.test(
      url.pathname,
    )
  )
    return null;
  return studioURL(`${url.pathname}${url.search}`, env);
}
