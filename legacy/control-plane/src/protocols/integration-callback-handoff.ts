import {
  createCipheriv,
  createDecipheriv,
  createHash,
  randomBytes,
} from "node:crypto";
import {
  isIntegrationCallbackPath,
  integrationResumeCookie,
} from "@webdock/api-client/integration-callbacks";
/** Encrypt OAuth codes/state before a Clerk handshake; no callback query leaves this cookie. */
export function createIntegrationCallbackHandoff(options: {
  origin: string;
  consoleOrigin: string;
  secret: string;
  now?: () => number;
}) {
  if (options.secret.length < 32)
    throw Error("Callback handoff encryption unavailable");
  const now = options.now ?? Date.now,
    key = createHash("sha256")
      .update("webdock-clerk-integration-resume-v1\0" + options.secret)
      .digest();
  const name = integrationResumeCookie(options.origin),
    aad = Buffer.from(options.origin);
  const cookie = (value: string, age: number) =>
    `${name}=${value}; Path=/; HttpOnly; SameSite=Lax; Max-Age=${age}${name.startsWith("__Host-") ? "; Secure" : ""}`;
  function target(value: unknown): value is string {
    if (
      typeof value !== "string" ||
      value.length > 2048 ||
      /[\x00-\x1f\x7f\\]/.test(value)
    )
      return false;
    const url = new URL(value, options.origin);
    return (
      value.startsWith("/") &&
      !value.startsWith("//") &&
      url.origin === options.origin &&
      !url.hash &&
      isIntegrationCallbackPath(url.pathname)
    );
  }
  return {
    clear: () => cookie("", 0),
    begin(request: Request): Response | null {
      const url = new URL(request.url),
        path = url.pathname + url.search;
      if (
        request.method !== "GET" ||
        url.origin !== options.origin ||
        !target(path)
      )
        return null;
      const nonce = randomBytes(12),
        cipher = createCipheriv("aes-256-gcm", key, nonce);
      cipher.setAAD(aad);
      const sealed = Buffer.concat([
        cipher.update(
          JSON.stringify({ target: path, expiresAt: now() + 180_000 }),
          "utf8",
        ),
        cipher.final(),
      ]);
      const value = [nonce, cipher.getAuthTag(), sealed]
        .map((part) => part.toString("base64url"))
        .join(".");
      return new Response(null, {
        status: 303,
        headers: {
          location: new URL("/auth/integration-resume", options.consoleOrigin)
            .href,
          "set-cookie": cookie(value, 180),
          "cache-control": "no-store",
          "referrer-policy": "no-referrer",
        },
      });
    },
    read(headers: Headers): string | null {
      try {
        const matches = (headers.get("cookie") ?? "")
          .split(";")
          .map((part) => part.trim())
          .filter((part) => part.startsWith(name + "="));
        if (matches.length !== 1) return null;
        const value = matches[0].slice(name.length + 1);
        if (value.length > 3500 || !/^[-\w]+\.[-\w]+\.[-\w]+$/.test(value))
          return null;
        const [nonce, tag, sealed] = value
          .split(".")
          .map((part) => Buffer.from(part, "base64url"));
        if (nonce.length !== 12 || tag.length !== 16) return null;
        const cipher = createDecipheriv("aes-256-gcm", key, nonce);
        cipher.setAAD(aad);
        cipher.setAuthTag(tag);
        const body = JSON.parse(
          Buffer.concat([cipher.update(sealed), cipher.final()]).toString(
            "utf8",
          ),
        ) as { target?: unknown; expiresAt?: unknown };
        return typeof body.expiresAt === "number" &&
          body.expiresAt > now() &&
          body.expiresAt <= now() + 180_000 &&
          target(body.target)
          ? body.target
          : null;
      } catch {
        return null;
      }
    },
  };
}
