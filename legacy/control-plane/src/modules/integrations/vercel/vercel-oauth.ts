import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import { sealData, unsealData } from "iron-session";

const FLOW_AGE = 600;
const CALLBACK_PATH = "/api/vercel/callback";
const SCOPES = new Set([
  "read:integration-configuration",
  "read:deployment",
  "read:domain",
]);
const FAILED = "Vercel connection could not be verified. Start again.";
export class VercelOAuthError extends Error {
  constructor(readonly stage: string) {
    super(FAILED);
    this.name = "VercelOAuthError";
  }
}
/** Vercel supplies this final hop to complete its popup installation transaction.
 * Never redirect a callback to a caller-controlled origin.
 */
export function vercelCompletionURL(value: string | null): string {
  if (!value || value.length > 8192 || /[\\\s\x00-\x1f\x7f]/u.test(value))
    throw new VercelOAuthError("completion-url");
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new VercelOAuthError("completion-url");
  }
  if (
    url.origin !== "https://vercel.com" ||
    url.username ||
    url.password ||
    url.hash
  )
    throw new VercelOAuthError("completion-url");
  return url.href;
}
const now = () => Math.floor(Date.now() / 1000);
const record = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === "object" && !Array.isArray(value);
const bounded = (value: unknown, max: number): value is string =>
  typeof value === "string" &&
  value.length > 0 &&
  value.length <= max &&
  !/[\s\x00-\x1f\x7f]/u.test(value);
const loopback = (hostname: string) =>
  ["localhost", "127.0.0.1", "[::1]"].includes(hostname);

type Options = {
  clientID: string;
  clientSecret: string;
  slug: string;
  origin: string;
  teamID: string;
  teamSlug: string;
  cookieSecret: string;
  fetcher?: (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;
};
type Flow = {
  kind: "vercel-integration-flow";
  clientID: string;
  operatorID: string;
  teamID: string;
  origin: string;
  slug: string;
  state: string;
  iat: number;
  exp: number;
};

/** Classic connectable-account integration, not Sign in with Vercel.
 * https://vercel.com/docs/integrations/create-integration/vercel-api-integrations
 * The route must clearCookie() after either success or failure; Vercel codes are single-use.
 */
export function configureVercelOAuth(options: Options) {
  const { clientID, clientSecret, slug, teamID, teamSlug, cookieSecret } =
    options;
  let origin: URL;
  try {
    origin = new URL(options.origin);
  } catch {
    throw Error("Invalid Vercel integration configuration.");
  }
  const local =
    process.env.NODE_ENV !== "production" &&
    origin.protocol === "http:" &&
    loopback(origin.hostname);
  if (
    (!local && origin.protocol !== "https:") ||
    origin.pathname !== "/" ||
    origin.search ||
    origin.hash ||
    origin.username ||
    origin.password ||
    !bounded(clientID, 256) ||
    !bounded(clientSecret, 4096) ||
    typeof slug !== "string" ||
    !/^[a-z0-9][a-z0-9-]{0,99}$/.test(slug) ||
    typeof teamID !== "string" ||
    !/^team_[A-Za-z0-9_-]{1,128}$/.test(teamID) ||
    typeof teamSlug !== "string" ||
    !/^[a-z0-9][a-z0-9-]{0,99}$/.test(teamSlug) ||
    typeof cookieSecret !== "string" ||
    cookieSecret.length < 32
  )
    throw Error("Invalid Vercel integration configuration.");
  const password = createHash("sha256")
    .update("webdock-vercel-integration-cookie\0")
    .update(cookieSecret)
    .digest("hex");
  const name = `${local ? "" : "__Host-"}webdock-vercel-flow`;
  const callbackURL = `${origin.origin}${CALLBACK_PATH}`;
  const fetcher = options.fetcher ?? fetch;
  const cookie = (value: string, age: number) =>
    `${name}=${value}; Path=/; HttpOnly;${local ? "" : " Secure;"} SameSite=Lax; Max-Age=${age}`;

  return {
    async start(operatorID: string): Promise<{ url: string; cookie: string }> {
      if (!bounded(operatorID, 256)) throw Error(FAILED);
      const iat = now();
      const flow: Flow = {
        kind: "vercel-integration-flow",
        clientID,
        operatorID,
        teamID,
        origin: origin.origin,
        slug,
        state: randomBytes(32).toString("base64url"),
        iat,
        exp: iat + FLOW_AGE,
      };
      const url = new URL(
        `https://vercel.com/${teamSlug}/~/integrations/${slug}`,
      );
      url.searchParams.set("state", flow.state);
      return {
        url: url.href,
        cookie: cookie(
          await sealData(flow, { password, ttl: FLOW_AGE }),
          FLOW_AGE,
        ),
      };
    },
    clearCookie(): string {
      return cookie("", 0);
    },
    async finish(
      request: Request,
      operatorID: string,
    ): Promise<{
      accessToken: string;
      teamID: string;
      configurationID: string;
      completionURL: string;
    }> {
      let stage = "callback-request";
      try {
        const incoming = new URL(request.url);
        const host = request.headers.get("host");
        // Next/proxies may normalize the URL to loopback. Only the configured raw Host
        // can attest that case; forwarded headers never authorize a callback.
        const proxyOrigin =
          host === origin.host &&
          (loopback(incoming.hostname) || incoming.host === origin.host) &&
          ["http:", "https:"].includes(incoming.protocol);
        if (
          request.method !== "GET" ||
          incoming.pathname !== CALLBACK_PATH ||
          incoming.hash ||
          incoming.username ||
          incoming.password ||
          (host !== null && host !== origin.host) ||
          (incoming.origin !== origin.origin && !proxyOrigin) ||
          !bounded(operatorID, 256)
        )
          throw Error(FAILED);
        stage = "callback-parameters";
        const params = incoming.searchParams;
        const names = [...params.keys()];
        if (new Set(names).size !== names.length || params.has("error"))
          throw Error(FAILED);
        const state = params.get("state"),
          code = params.get("code"),
          configurationID = params.get("configurationId");
        if (
          !state ||
          !/^[A-Za-z0-9_-]{43}$/.test(state) ||
          !bounded(code, 512) ||
          !configurationID ||
          !/^icfg_[A-Za-z0-9_-]{1,128}$/.test(configurationID) ||
          params.get("teamId") !== teamID
        )
          throw Error(FAILED);
        stage = "completion-url";
        const completionURL = vercelCompletionURL(params.get("next"));
        stage = "flow-cookie";
        const cookies = (request.headers.get("cookie") || "")
          .split(";")
          .map((value) => value.trim())
          .filter((value) => value.startsWith(`${name}=`));
        if (cookies.length !== 1 || cookies[0]!.length > 4096)
          throw Error(FAILED);
        stage = "flow-validation";
        const flow = await unsealData<unknown>(
          cookies[0]!.slice(name.length + 1),
          { password, ttl: FLOW_AGE },
        );
        const timestamp = now();
        if (
          !record(flow) ||
          flow.kind !== "vercel-integration-flow" ||
          flow.clientID !== clientID ||
          flow.teamID !== teamID ||
          flow.operatorID !== operatorID ||
          flow.origin !== origin.origin ||
          flow.slug !== slug ||
          !Number.isSafeInteger(flow.iat) ||
          !Number.isSafeInteger(flow.exp) ||
          (flow.iat as number) > timestamp ||
          (flow.iat as number) < timestamp - FLOW_AGE ||
          flow.exp !== (flow.iat as number) + FLOW_AGE ||
          (flow.exp as number) <= timestamp ||
          typeof flow.state !== "string" ||
          !/^[A-Za-z0-9_-]{43}$/.test(flow.state) ||
          !timingSafeEqual(Buffer.from(state), Buffer.from(flow.state))
        )
          throw Error(FAILED);
        stage = "token-exchange";
        const exchanged = await fetcher(
          "https://api.vercel.com/v2/oauth/access_token",
          {
            method: "POST",
            headers: {
              Accept: "application/json",
              "Content-Type": "application/x-www-form-urlencoded",
            },
            body: new URLSearchParams({
              client_id: clientID,
              client_secret: clientSecret,
              code,
              redirect_uri: callbackURL,
            }),
            redirect: "error",
            cache: "no-store",
            signal: AbortSignal.timeout(10000),
          },
        );
        if (!exchanged.ok) throw Error(FAILED);
        stage = "token-response";
        const token: unknown = await exchanged.json();
        if (
          !record(token) ||
          token.error ||
          !bounded(token.access_token, 8192) ||
          !/^[A-Za-z0-9._~+/-]+=*$/.test(token.access_token) ||
          typeof token.token_type !== "string" ||
          token.token_type.toLowerCase() !== "bearer" ||
          token.installation_id !== configurationID ||
          token.team_id !== teamID
        )
          throw Error(FAILED);
        stage = "configuration-read";
        const url = new URL(
          `https://api.vercel.com/v1/integrations/configuration/${configurationID}`,
        );
        url.searchParams.set("teamId", teamID);
        const response = await fetcher(url.href, {
          headers: {
            Accept: "application/json",
            Authorization: `Bearer ${token.access_token}`,
          },
          redirect: "error",
          cache: "no-store",
          signal: AbortSignal.timeout(10000),
        });
        if (!response.ok) throw Error(FAILED);
        stage = "configuration-validation";
        const configuration: unknown = await response.json();
        const grantedScopes =
          record(configuration) && Array.isArray(configuration.scopes)
            ? configuration.scopes
            : [];
        if (
          !record(configuration) ||
          configuration.id !== configurationID ||
          configuration.teamId !== teamID ||
          configuration.integrationId !== clientID ||
          configuration.slug !== slug ||
          configuration.disabledAt != null ||
          configuration.deletedAt != null ||
          configuration.deleteRequestedAt != null ||
          (configuration.status !== undefined &&
            !["onboarding", "pending", "ready", "resumed"].includes(
              String(configuration.status),
            )) ||
          (configuration.installationType !== undefined &&
            configuration.installationType !== "external") ||
          !Array.isArray(configuration.scopes) ||
          configuration.scopes.length !== SCOPES.size + 1 ||
          new Set(configuration.scopes).size !== configuration.scopes.length ||
          [...SCOPES].some((scope) => !grantedScopes.includes(scope)) ||
          configuration.scopes.some(
            (scope) =>
              typeof scope !== "string" ||
              (!SCOPES.has(scope) &&
                scope !== "read:project" &&
                scope !== "read-write:project"),
          )
        )
          throw Error(FAILED);
        return {
          accessToken: token.access_token,
          teamID,
          configurationID,
          completionURL,
        };
      } catch {
        // Only a fixed stage label crosses this boundary, never credentials or provider text.
        throw new VercelOAuthError(stage);
      }
    },
  };
}
