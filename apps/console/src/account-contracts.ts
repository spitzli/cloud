import { z } from "zod";
import { sessionSchema } from "./api";
export const nativeSessionSchema = sessionSchema.shape.data;
export const resultSchema = z.object({
  status: z.boolean().optional(),
  twoFactorRedirect: z.boolean().optional(),
  redirect: z.union([z.string(), z.boolean()]).optional(),
});
export const sessionListSchema = z.array(
  z.object({
    id: z.string(),
    current: z.boolean(),
    expiresAt: z.string(),
    createdAt: z.string(),
  }),
);
export const passkeyListSchema = z.array(
  z.object({
    id: z.string(),
    name: z.string().nullable(),
    createdAt: z.string().nullable(),
  }),
);
export function authorizationContinuation(
  raw: string | null,
  origin: string,
): string | null {
  if (
    !raw ||
    raw.length > 8192 ||
    !raw.startsWith("/") ||
    raw.startsWith("//") ||
    hasUnsafeCharacter(raw)
  )
    return null;
  try {
    const url = new URL(raw, origin);
    return url.origin === origin &&
      url.pathname === "/api/auth/oauth2/authorize"
      ? url.pathname + url.search
      : null;
  } catch {
    return null;
  }
}

function hasUnsafeCharacter(value: string): boolean {
  for (const char of value) {
    if (char === "\\" || char.charCodeAt(0) <= 32) return true;
  }
  return false;
}
