import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import { sealData, unsealData } from "iron-session";

const AGE = 8 * 60 * 60;
const FLOW_AGE = 600;
const ARCHIVE_LIMIT = 32 * 1024 * 1024;
const now = () => Math.floor(Date.now() / 1000);
const record = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === "object";
const positive = (value: unknown): value is number =>
  Number.isSafeInteger(value) && Number(value) > 0;
const loginName = (value: unknown): value is string =>
  typeof value === "string" && /^[a-zA-Z0-9-]{1,100}$/.test(value);
export const githubReturnPath = (value: unknown): value is string =>
  typeof value === "string" &&
  /^(?:\/integrations|\/plugins(?:\/[1-9][0-9]{0,18}(?:\/install)?)?)$/.test(
    value,
  );
export type GitHubSession = {
  kind: "github-session";
  clientID: string;
  operatorID: string;
  accessToken: string;
  login: string;
  exp: number;
};
export type GitHubInstallation = { id: number; account: string };
export type GitHubRepository = {
  id: number;
  name: string;
  url: string;
  private: boolean;
};
export type GitHubSelection = {
  installation: number;
  installationPage: number;
  page: number;
  repository: number;
};
type Options = {
  clientID: string;
  clientSecret: string;
  appSlug: string;
  origin: string;
  cookieSecret: string;
};

export function githubOrigin() {
  return (
    process.env.WEBDOCK_SSO_APP_ORIGIN ||
    process.env.WEBDOCK_STUDIO_ORIGIN ||
    "http://localhost:3120"
  );
}
export function getGitHub() {
  const clientID = process.env.WEBDOCK_GITHUB_CLIENT_ID;
  const clientSecret = process.env.WEBDOCK_GITHUB_CLIENT_SECRET;
  const appSlug = process.env.WEBDOCK_GITHUB_APP_SLUG;
  const cookieSecret = process.env.WEBDOCK_SSO_COOKIE_SECRET;
  if (!clientID || !clientSecret || !appSlug || !cookieSecret) return null;
  try {
    return configureGitHub({
      clientID,
      clientSecret,
      appSlug,
      cookieSecret,
      origin: githubOrigin(),
    });
  } catch {
    return null;
  }
}

export function configureGitHub(options: Options) {
  const origin = new URL(options.origin);
  const local =
    process.env.NODE_ENV !== "production" &&
    origin.protocol === "http:" &&
    ["localhost", "127.0.0.1"].includes(origin.hostname);
  if (
    (!local && origin.protocol !== "https:") ||
    origin.pathname !== "/" ||
    origin.search ||
    origin.hash ||
    origin.username ||
    origin.password ||
    options.cookieSecret.length < 32 ||
    !options.clientID ||
    !options.clientSecret ||
    !/^[a-z0-9-]{1,100}$/.test(options.appSlug)
  )
    throw Error("Invalid GitHub App configuration");
  // Separate purpose/key from the existing SSO cookies, without another persisted secret.
  const password = createHash("sha256")
    .update("webdock-github-cookie\0")
    .update(options.cookieSecret)
    .digest("hex");
  const SESSION = `${local ? "" : "__Host-"}webdock-github`;
  const FLOW = `${SESSION}-flow`;
  const callbackURL = `${origin.origin}/api/github/callback`;
  const cookie = (name: string, value: string, age: number) =>
    `${name}=${value}; Path=/; HttpOnly;${local ? "" : " Secure;"} SameSite=Lax; Max-Age=${age}`;
  const redirect = (status: string, returnTo = "/integrations") =>
    new Response(null, {
      status: 303,
      headers: {
        Location: `${origin.origin}${githubReturnPath(returnTo) ? returnTo : "/integrations"}?github=${status}`,
        "Cache-Control": "no-store",
        "Referrer-Policy": "no-referrer",
      },
    });
  const clear = (response: Response, name: string) =>
    response.headers.append("Set-Cookie", cookie(name, "", 0));
  const sameOrigin = (request: Request) =>
    request.method === "POST" &&
    request.headers.get("origin") === origin.origin;
  async function read(headers: Headers, name: string, age: number) {
    const values = (headers.get("cookie") || "")
      .split(";")
      .map((c) => c.trim())
      .filter((c) => c.startsWith(`${name}=`));
    if (values.length !== 1) return null;
    try {
      const value = await unsealData<unknown>(
        values[0]!.slice(name.length + 1),
        { password, ttl: age },
      );
      return record(value) &&
        positive(value.exp) &&
        value.exp > now() &&
        value.exp <= now() + age &&
        value.clientID === options.clientID
        ? value
        : null;
    } catch {
      return null;
    }
  }
  async function api(path: string, token: string) {
    const response = await fetch(`https://api.github.com${path}`, {
      headers: {
        Accept: "application/vnd.github+json",
        Authorization: `Bearer ${token}`,
        "X-GitHub-Api-Version": "2026-03-10",
      },
      redirect: "error",
      cache: "no-store",
      signal: AbortSignal.timeout(10000),
    });
    if (!response.ok)
      throw Error(
        "GitHub access is unavailable. Reconnect or check the app installation.",
      );
    const result: unknown = await response.json();
    if (!record(result)) throw Error("Invalid GitHub response");
    return result;
  }
  const pageNumber = (page: number) => {
    if (!positive(page) || page > 10000) throw Error("Invalid page");
    return page;
  };
  async function installations(
    session: GitHubSession,
    page = 1,
    contentsFor?: number,
  ) {
    if (session.exp <= now())
      throw Error("GitHub connection expired. Connect again.");
    const result = await api(
      `/user/installations?per_page=50&page=${pageNumber(page)}`,
      session.accessToken,
    );
    if (
      !Array.isArray(result.installations) ||
      !Number.isSafeInteger(result.total_count)
    )
      throw Error("Invalid GitHub installations");
    const items: GitHubInstallation[] = [];
    for (const installation of result.installations) {
      if (
        !record(installation) ||
        installation.app_slug !== options.appSlug ||
        installation.suspended_at
      )
        continue;
      if (
        !record(installation.permissions) ||
        installation.permissions.metadata !== "read" ||
        Object.entries(installation.permissions).some(
          ([key, value]) =>
            key !== "metadata" &&
            value !== "none" &&
            !(key === "contents" && value === "read") &&
            !(key === "actions" && (value === "read" || value === "write")) &&
            !(key === "checks" && value === "write"),
        )
      )
        throw Error(
          "Configure the GitHub App with Metadata: read-only and optionally Contents: read-only. Existing Hosting or build Actions: read/write and Checks: write permissions may remain; unrelated permissions are not supported.",
        );
      if (
        installation.id === contentsFor &&
        installation.permissions.contents !== "read"
      )
        throw Error(
          "Approve Contents: read-only access for this GitHub installation.",
        );
      if (
        !positive(installation.id) ||
        !record(installation.account) ||
        !loginName(installation.account.login)
      )
        throw Error("Invalid GitHub installation");
      items.push({ id: installation.id, account: installation.account.login });
    }
    return {
      items,
      page,
      totalPages: Math.max(1, Math.ceil(Number(result.total_count) / 50)),
    };
  }
  async function repositories(
    session: GitHubSession,
    installation: number,
    installationPage = 1,
    page = 1,
    requireContents = false,
  ) {
    if (!positive(installation)) throw Error("Invalid installation");
    const available = await installations(
      session,
      installationPage,
      requireContents ? installation : undefined,
    );
    if (!available.items.some((item) => item.id === installation))
      throw Error("This GitHub installation is no longer accessible.");
    const result = await api(
      `/user/installations/${installation}/repositories?per_page=50&page=${pageNumber(page)}`,
      session.accessToken,
    );
    if (
      !Array.isArray(result.repositories) ||
      !Number.isSafeInteger(result.total_count)
    )
      throw Error("Invalid GitHub repositories");
    const items = result.repositories.map((repo: unknown): GitHubRepository => {
      if (
        !record(repo) ||
        !positive(repo.id) ||
        typeof repo.full_name !== "string" ||
        !/^[a-zA-Z0-9-]+\/[a-zA-Z0-9_.-]+$/.test(repo.full_name) ||
        repo.html_url !== `https://github.com/${repo.full_name}` ||
        typeof repo.private !== "boolean"
      )
        throw Error("Invalid GitHub repository");
      return {
        id: repo.id,
        name: repo.full_name,
        url: repo.html_url,
        private: repo.private,
      };
    });
    return {
      items,
      page,
      totalPages: Math.max(1, Math.ceil(Number(result.total_count) / 50)),
    };
  }
  async function repository(
    session: GitHubSession,
    selection: GitHubSelection,
    requireContents = false,
  ) {
    const result = await repositories(
      session,
      selection.installation,
      selection.installationPage,
      selection.page,
      requireContents,
    );
    const repository = result.items.find(
      (item) => item.id === selection.repository,
    );
    if (!repository)
      throw Error(
        "This repository is no longer available in the selected installation.",
      );
    return repository;
  }
  async function session(
    headers: Headers,
    operatorID: string,
  ): Promise<GitHubSession | null> {
    const value = await read(headers, SESSION, AGE);
    return value?.kind === "github-session" &&
      value.operatorID === operatorID &&
      loginName(value.login) &&
      typeof value.accessToken === "string" &&
      /^ghu_[A-Za-z0-9_]{1,250}$/.test(value.accessToken)
      ? (value as GitHubSession)
      : null;
  }
  return {
    callbackURL,
    installURL: `https://github.com/apps/${options.appSlug}/installations/new`,
    permissionsURL: `https://github.com/settings/connections/applications/${options.clientID}`,
    installations,
    repositories,
    repository,
    session,
    async source(
      headers: Headers,
      operatorID: string,
      selection: GitHubSelection,
      ref = "HEAD",
    ) {
      if (
        typeof ref !== "string" ||
        !/^[A-Za-z0-9][A-Za-z0-9._\/@+-]{0,254}$/.test(ref) ||
        ref.includes("..") ||
        ref.endsWith("/") ||
        ref.includes("//")
      )
        throw Error("Invalid GitHub ref.");
      try {
        const authorized = await session(headers, operatorID);
        if (!authorized) throw Error("GitHub connection expired.");
        const selected = await repository(authorized, selection, true);
        const resolved = await api(
          `/repos/${selected.name}/commits/${encodeURIComponent(ref)}`,
          authorized.accessToken,
        );
        if (
          typeof resolved.sha !== "string" ||
          !/^[a-f0-9]{40}$/.test(resolved.sha)
        )
          throw Error("Invalid GitHub commit.");
        const response = await fetch(
          `https://api.github.com/repos/${selected.name}/zipball/${resolved.sha}`,
          {
            headers: {
              Accept: "application/vnd.github+json",
              Authorization: `Bearer ${authorized.accessToken}`,
              "X-GitHub-Api-Version": "2026-03-10",
            },
            redirect: "manual",
            cache: "no-store",
            signal: AbortSignal.timeout(10000),
          },
        );
        await response.body?.cancel();
        if (response.status !== 302) throw Error("GitHub archive unavailable.");
        const location = new URL(response.headers.get("location") ?? "");
        if (
          location.origin !== "https://codeload.github.com" ||
          location.username ||
          location.password ||
          location.hash
        )
          throw Error("Invalid GitHub archive destination.");
        // The signed download location grants access; never forward the user's token.
        const download = await fetch(location.href, {
          redirect: "error",
          cache: "no-store",
          signal: AbortSignal.timeout(30000),
        });
        const length = download.headers.get("content-length");
        if (
          !download.ok ||
          !download.body ||
          (length !== null &&
            (!/^\d+$/.test(length) || Number(length) > ARCHIVE_LIMIT))
        ) {
          await download.body?.cancel();
          throw Error("GitHub archive unavailable or too large.");
        }
        const reader = download.body.getReader();
        const chunks: Uint8Array[] = [];
        let size = 0;
        try {
          for (;;) {
            const { done, value } = await reader.read();
            if (done) break;
            size += value.byteLength;
            if (size > ARCHIVE_LIMIT) {
              await reader.cancel();
              throw Error("GitHub archive too large.");
            }
            chunks.push(value);
          }
        } finally {
          reader.releaseLock();
        }
        if (!size) throw Error("GitHub archive is empty.");
        return {
          repository: selected,
          commit: resolved.sha,
          archive: new Uint8Array(Buffer.concat(chunks, size)),
        };
      } catch {
        // Provider errors can contain tokens or signed archive locations.
        throw Error(
          "GitHub source is unavailable. Reconnect and approve Contents: read-only for the selected repository; archives must be at most 32 MiB.",
        );
      }
    },
    async connect(
      request: Request,
      operatorID: string,
      returnTo = "/integrations",
    ) {
      if (!sameOrigin(request)) return new Response(null, { status: 403 });
      if (!githubReturnPath(returnTo))
        return new Response(null, { status: 400 });
      const verifier = randomBytes(32).toString("base64url");
      const state = randomBytes(32).toString("base64url");
      const flow = {
        kind: "github-flow",
        clientID: options.clientID,
        operatorID,
        state,
        verifier,
        returnTo,
        exp: now() + FLOW_AGE,
      };
      const url = new URL("https://github.com/login/oauth/authorize");
      url.search = new URLSearchParams({
        client_id: options.clientID,
        redirect_uri: callbackURL,
        state,
        code_challenge: createHash("sha256")
          .update(verifier)
          .digest("base64url"),
        code_challenge_method: "S256",
      }).toString();
      return new Response(null, {
        status: 303,
        headers: {
          Location: url.href,
          "Cache-Control": "no-store",
          "Referrer-Policy": "no-referrer",
          "Set-Cookie": cookie(
            FLOW,
            await sealData(flow, { password, ttl: FLOW_AGE }),
            FLOW_AGE,
          ),
        },
      });
    },
    async callback(request: Request, operatorID: string) {
      let response: Response;
      let returnTo = "/integrations";
      try {
        const incoming = new URL(request.url);
        if (
          request.method !== "GET" ||
          incoming.pathname !== "/api/github/callback" ||
          (incoming.host !== origin.host &&
            request.headers.get("host") !== origin.host)
        )
          throw Error("Invalid callback");
        const flow = await read(request.headers, FLOW, FLOW_AGE);
        const state = incoming.searchParams.get("state");
        const code = incoming.searchParams.get("code");
        if (
          !flow ||
          flow.kind !== "github-flow" ||
          flow.operatorID !== operatorID ||
          typeof flow.state !== "string" ||
          typeof flow.verifier !== "string" ||
          incoming.searchParams.getAll("state").length !== 1 ||
          !state ||
          state.length !== flow.state.length ||
          !timingSafeEqual(Buffer.from(state), Buffer.from(flow.state))
        )
          throw Error("Invalid authorization flow");
        if (flow.returnTo !== undefined && !githubReturnPath(flow.returnTo))
          throw Error("Invalid GitHub return path");
        returnTo =
          flow.returnTo === undefined
            ? "/integrations"
            : (flow.returnTo as string);
        if (
          incoming.searchParams.getAll("code").length !== 1 ||
          !code ||
          code.length > 512 ||
          incoming.searchParams.has("error")
        )
          throw Error("Invalid authorization response");
        const result = await fetch(
          "https://github.com/login/oauth/access_token",
          {
            method: "POST",
            headers: {
              Accept: "application/json",
              "Content-Type": "application/x-www-form-urlencoded",
            },
            body: new URLSearchParams({
              client_id: options.clientID,
              client_secret: options.clientSecret,
              code,
              redirect_uri: callbackURL,
              code_verifier: flow.verifier,
            }),
            redirect: "error",
            cache: "no-store",
            signal: AbortSignal.timeout(10000),
          },
        );
        const token: unknown = await result.json();
        if (
          !result.ok ||
          !record(token) ||
          typeof token.access_token !== "string" ||
          !/^ghu_[A-Za-z0-9_]{1,250}$/.test(token.access_token) ||
          token.token_type !== "bearer" ||
          token.scope !== "" ||
          !positive(token.expires_in) ||
          token.expires_in > AGE
        )
          throw Error("Invalid GitHub user token");
        const user = await api("/user", token.access_token);
        if (!positive(user.id) || !loginName(user.login))
          throw Error("Invalid GitHub account");
        const session: GitHubSession = {
          kind: "github-session",
          clientID: options.clientID,
          operatorID,
          login: user.login,
          accessToken: token.access_token,
          exp: now() + token.expires_in,
        };
        response = redirect("connected", returnTo);
        response.headers.append(
          "Set-Cookie",
          cookie(
            SESSION,
            await sealData(session, { password, ttl: AGE }),
            token.expires_in,
          ),
        );
      } catch {
        response = redirect("failed", returnTo);
      }
      clear(response, FLOW);
      return response;
    },
    disconnect(request: Request, returnTo = "/integrations") {
      if (!sameOrigin(request)) return new Response(null, { status: 403 });
      if (!githubReturnPath(returnTo))
        return new Response(null, { status: 400 });
      const response = redirect("disconnected", returnTo);
      clear(response, SESSION);
      clear(response, FLOW);
      return response;
    },
  };
}
