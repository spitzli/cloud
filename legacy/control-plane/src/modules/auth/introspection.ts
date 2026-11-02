import { requireOAuthCapability } from "@webdock/auth-configuration/protocols";
import { AsyncLocalStorage } from "node:async_hooks";

type Handler = (request: Request) => Promise<Response>;
const requests = new AsyncLocalStorage<{
  active: boolean;
  results: Map<string, { handler: Handler; response: Promise<Response> }>;
}>();

/** Only an individual portal dispatch may share authorization work. */
export function withTokenIntrospection<T>(run: () => Promise<T>): Promise<T> {
  const scope = { active: true, results: new Map() };
  return requests.run(scope, async () => {
    try {
      return await run();
    } finally {
      scope.active = false;
      scope.results.clear();
    }
  });
}

export async function introspectToken(
  token: string,
  clientID: string,
  clientSecret: string,
  handler?: Handler,
): Promise<Response> {
  if (!handler) requireOAuthCapability();
  handler ??= (await import("./auth")).auth.handler;
  const origin = new URL(process.env.BETTER_AUTH_URL || "http://localhost:3125")
    .origin;
  const body = new URLSearchParams({
    token,
    token_type_hint: "access_token",
    client_id: clientID,
    client_secret: clientSecret,
  });
  const key = JSON.stringify([origin, token, clientID, clientSecret]);
  const scope = requests.getStore();
  const cached = scope?.active ? scope.results.get(key) : undefined;
  if (cached?.handler === handler) return (await cached.response).clone();
  const response = handler(
    new Request(origin + "/api/auth/oauth2/introspect", {
      method: "POST",
      headers: {
        Origin: origin,
        "Content-Type": "application/x-www-form-urlencoded",
      },
      body,
    }),
  );
  const entry = { handler, response };
  if (scope?.active) scope.results.set(key, entry);
  try {
    const result = await response;
    if (!result.ok && scope?.results.get(key) === entry)
      scope.results.delete(key);
    return result.clone();
  } catch (error) {
    if (scope?.results.get(key) === entry) scope.results.delete(key);
    throw error;
  }
}
