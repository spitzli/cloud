import { constants } from "node:fs";
import { open } from "node:fs/promises";
import { databaseURL } from "../../legacy/control-plane/src/db/client";
export class OperatorSetupError extends Error {}
const fail = (message: string): never => {
  throw new OperatorSetupError(message);
};
export function operatorConfig(
  env: Record<string, string | undefined>,
  target: string | undefined,
) {
  if (!env.DATABASE_URL_UNPOOLED)
    fail("Explicit offline DATABASE_URL_UNPOOLED required");
  let url: URL;
  try {
    url = databaseURL(env.DATABASE_URL_UNPOOLED!);
  } catch {
    fail("Invalid offline database URL");
  }
  if (target !== `${url!.hostname}:${url!.port || 5432}${url!.pathname}`)
    fail("Pass the exact operator setup host:port/database");
  if (decodeURIComponent(url!.username) !== "webdock_offline_migrate")
    fail("Use the dedicated offline migrator for operator setup");
  const local = ["127.0.0.1", "localhost"].includes(url!.hostname);
  if (!local && !env.DATABASE_CA_CERT)
    fail("Remote operator setup requires a verified CA");
  return { url: url!, local, ca: env.DATABASE_CA_CERT };
}
export async function readPrivateInput(path: string) {
  const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const stat = await file.stat();
    if (
      !stat.isFile() ||
      stat.size > 16384 ||
      (stat.mode & 0o077) !== 0 ||
      (typeof process.getuid === "function" && stat.uid !== process.getuid())
    )
      fail(
        "Input must be an owned private regular file (mode 0600 or stricter)",
      );
    try {
      return JSON.parse(await file.readFile("utf8")) as unknown;
    } catch {
      fail("Input must contain valid JSON");
    }
  } finally {
    await file.close();
  }
}
function origin(value: unknown, local: boolean): string {
  if (typeof value !== "string") return fail("Canonical origins are required");
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return fail("Invalid canonical origin");
  }
  if (
    value !== url.origin ||
    url.username ||
    url.password ||
    (url.protocol !== "https:" &&
      !(
        local &&
        url.protocol === "http:" &&
        ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname)
      ))
  )
    fail(
      "Use exact HTTPS canonical origins (loopback HTTP only with a loopback database)",
    );
  return url.origin;
}
export function operatorInput(
  value: unknown,
  env: Record<string, string | undefined>,
  local: boolean,
) {
  if (!value || typeof value !== "object" || Array.isArray(value))
    fail("Invalid initial operator input");
  const data = value as Record<string, unknown>;
  if (
    Object.keys(data).some(
      (key) =>
        ![
          "email",
          "name",
          "password",
          "authOrigin",
          "studioOrigin",
          "consoleOrigin",
        ].includes(key),
    )
  )
    fail("Unknown initial operator input field");
  if (
    typeof data.name !== "string" ||
    !data.name.trim() ||
    data.name.length > 160 ||
    /[\x00-\x1f]/.test(data.name)
  )
    fail("A valid operator name is required");
  if (
    typeof data.email !== "string" ||
    data.email.length > 254 ||
    !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(data.email) ||
    /[\x00-\x1f]/.test(data.email)
  )
    fail("A valid operator email is required");
  if (
    data.password !== undefined &&
    env.WEBDOCK_INITIAL_OPERATOR_PASSWORD !== undefined
  )
    fail("Supply the password in the private file or environment, never both");
  const password = data.password ?? env.WEBDOCK_INITIAL_OPERATOR_PASSWORD;
  if (
    typeof password !== "string" ||
    password.length < 16 ||
    password.length > 128 ||
    /[\x00-\x1f]/.test(password)
  )
    fail(
      "Initial password must contain 16–128 characters without control characters",
    );
  const authOrigin = origin(data.authOrigin, local),
    studioOrigin = origin(data.studioOrigin, local),
    consoleOrigin = origin(data.consoleOrigin, local);
  if (new Set([authOrigin, studioOrigin, consoleOrigin]).size !== 3)
    fail("Auth, Studio protocol and Console origins must be distinct");
  for (const [key, expected] of [
    ["BETTER_AUTH_URL", authOrigin],
    ["WEBDOCK_STUDIO_ORIGIN", studioOrigin],
    ["WEBDOCK_CONSOLE_ORIGIN", consoleOrigin],
  ])
    if (env[key] !== expected)
      fail(
        "Input origins must exactly match BETTER_AUTH_URL, WEBDOCK_STUDIO_ORIGIN and WEBDOCK_CONSOLE_ORIGIN",
      );
  return {
    email: (data.email as string).toLowerCase(),
    name: (data.name as string).trim(),
    password: password as string,
    authOrigin,
    studioOrigin,
    consoleOrigin,
  };
}
export type InitialOperatorInput = ReturnType<typeof operatorInput>;
