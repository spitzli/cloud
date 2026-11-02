import { AsyncLocalStorage } from "node:async_hooks";
import type { Site } from "./studio-contracts";
const requests = new AsyncLocalStorage<{
  native: Headers;
  browser: Headers;
  accessToken?: string;
}>();
export function withNativeHeaders<T>(
  headers: Headers,
  run: () => T,
  browser = headers,
  accessToken?: string,
): T {
  return requests.run({ native: headers, browser, accessToken }, run);
}
export function nativeHeaders() {
  const headers = requests.getStore();
  if (!headers) throw Error("Missing verified request context");
  return headers.native;
}
export async function portalCall<T>(
  operation: string,
  args: unknown[] = [],
): Promise<T> {
  const headers = new Headers(nativeHeaders());
  headers.set("content-type", "application/json");
  const { handleNativeStudioRequest } = await import("./studio-api");
  const response = await handleNativeStudioRequest(
    new Request(
      new URL(
        "/v1/auth/studio",
        process.env.BETTER_AUTH_URL || "http://localhost:3125",
      ),
      { method: "POST", headers, body: JSON.stringify({ operation, args }) },
    ),
  );
  const result = (await response.json()) as {
    data?: T;
    error?: { message?: string };
  };
  if (!response.ok) throw Error("Account access is unavailable");
  return result.data as T;
}
export const getStudioSession = () =>
  portalCall<{
    user: { id: string };
    operator: boolean;
    preview?: {
      readOnly: true;
      status: "active" | "expired";
      customerID: string;
    };
  }>("session");

export function browserHeaders() {
  const value = requests.getStore();
  if (!value) throw Error("Missing verified request context");
  return value.browser;
}
export function delegatedToken() {
  const value = requests.getStore()?.accessToken;
  if (!value) throw Error("A delegated Studio session is required");
  return value;
}
