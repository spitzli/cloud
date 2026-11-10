/** Vercel adapter. Response allowlists intentionally exclude environment values and credentials.
 * https://vercel.com/docs/rest-api/projects/find-a-project-by-id-or-name
 * https://vercel.com/docs/rest-api/deployments/list-deployments
 * https://vercel.com/docs/rest-api/projects/retrieve-project-domains-by-project-by-id-or-name
 */
import type {
  VercelAPIErrorCode,
  VercelDeploymentStatus,
  VercelDeployment,
  VercelDomain,
  VercelProjectSnapshot,
} from "@spitzli/hosting-provider-contracts";
export type {
  VercelAPIErrorCode,
  VercelDeploymentStatus,
  VercelDeployment,
  VercelDomain,
  VercelProjectSnapshot,
} from "@spitzli/hosting-provider-contracts";
const messages: Record<VercelAPIErrorCode, string> = {
  unavailable: "Vercel project information is temporarily unavailable.",
  disconnected: "Reconnect the Vercel integration to read project information.",
  forbidden:
    "The Vercel integration does not have access to this project or team.",
  not_found: "The mapped Vercel project could not be found.",
  deletion_forbidden:
    "Project deletion is unavailable: the Vercel integration needs project write permission for this team and project.",
};
export class VercelAPIError extends Error {
  constructor(public readonly code: VercelAPIErrorCode) {
    super(messages[code]);
    this.name = "VercelAPIError";
  }
}
const unavailable = () => new VercelAPIError("unavailable");
function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw unavailable();
  return value as Record<string, unknown>;
}
function text(value: unknown, maximum: number): string | null {
  if (typeof value !== "string") return null;
  const clean = value
    .replace(/[\u0000-\u001f\u007f\u202a-\u202e\u2066-\u2069<>]/g, " ")
    .trim()
    .slice(0, maximum);
  return clean || null;
}
function date(value: unknown): string | null {
  return typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value > 0 &&
    value <= 8.64e15
    ? new Date(value).toISOString()
    : null;
}
function domain(value: unknown, wildcard = false): string | null {
  if (typeof value !== "string" || value.length > 253) return null;
  const name = value.toLowerCase();
  const host = wildcard && name.startsWith("*.") ? name.slice(2) : name;
  const labels = host.split(".");
  if (
    labels.length < 2 ||
    labels.some(
      (label) => !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(label),
    ) ||
    !/^[a-z][a-z0-9-]*$/.test(labels.at(-1) || "") ||
    /\.(?:localhost|local|internal|invalid|test)$/.test(host)
  )
    return null;
  return name;
}
function deploymentURL(value: unknown): string | null {
  if (typeof value !== "string" || value.length > 300 || /[\s\\]/.test(value))
    return null;
  try {
    const url = new URL(
      value.startsWith("https://") ? value : `https://${value}`,
    );
    if (
      url.protocol !== "https:" ||
      url.username ||
      url.password ||
      url.port ||
      url.search ||
      url.hash ||
      url.pathname !== "/" ||
      !domain(url.hostname)
    )
      return null;
    // Reject malformed input that the URL parser would silently normalize.
    if (
      value !== url.hostname &&
      value !== `https://${url.hostname}` &&
      value !== url.href
    )
      return null;
    return url.href;
  } catch {
    return null;
  }
}
function deployment(value: unknown, projectID: string): VercelDeployment {
  const data = record(value),
    id = data.uid ?? data.id;
  if (typeof id !== "string" || !/^dpl_[a-zA-Z0-9]{1,128}$/.test(id))
    throw unavailable();
  if (data.projectId !== undefined && data.projectId !== projectID)
    throw new VercelAPIError("forbidden");
  const meta =
    data.meta && typeof data.meta === "object" && !Array.isArray(data.meta)
      ? (data.meta as Record<string, unknown>)
      : {};
  const sha =
    meta.githubCommitSha ?? meta.gitlabCommitSha ?? meta.bitbucketCommitSha;
  const state = data.readyState ?? data.state;
  const states: VercelDeploymentStatus[] = [
    "QUEUED",
    "INITIALIZING",
    "BUILDING",
    "READY",
    "ERROR",
    "CANCELED",
    "BLOCKED",
  ];
  return {
    id,
    status: states.includes(state as VercelDeploymentStatus)
      ? (state as VercelDeploymentStatus)
      : "UNKNOWN",
    url: deploymentURL(data.url),
    target: text(data.target, 100),
    createdAt: date(data.createdAt ?? data.created),
    readyAt: date(data.readyAt ?? data.ready),
    branch: text(
      meta.githubCommitRef ?? meta.gitlabCommitRef ?? meta.bitbucketCommitRef,
      200,
    ),
    commitSHA:
      typeof sha === "string" && /^[a-fA-F0-9]{7,64}$/.test(sha) ? sha : null,
    commitMessage: text(
      meta.githubCommitMessage ??
        meta.gitlabCommitMessage ??
        meta.bitbucketCommitMessage,
      500,
    ),
  };
}
async function readJSON(response: Response): Promise<unknown> {
  const maximum = 4 * 1024 * 1024;
  if (
    Number(response.headers.get("content-length")) > maximum ||
    !response.body
  )
    throw unavailable();
  const reader = response.body.getReader(),
    chunks: Uint8Array[] = [];
  let length = 0;
  try {
    while (true) {
      const part = await reader.read();
      if (part.done) break;
      length += part.value.byteLength;
      if (length > maximum) {
        await reader.cancel();
        throw unavailable();
      }
      chunks.push(part.value);
    }
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } finally {
    reader.releaseLock();
  }
}

type VercelReadOptions = {
  token: string;
  teamID: string;
  fetcher?: typeof fetch;
};
function vercelReader({ token, teamID, fetcher = fetch }: VercelReadOptions) {
  if (
    typeof token !== "string" ||
    !token ||
    token.length > 4096 ||
    /[^\x21-\x7e]/.test(token)
  )
    throw new VercelAPIError("disconnected");
  if (typeof teamID !== "string" || !/^team_[A-Za-z0-9]{1,128}$/.test(teamID))
    throw new VercelAPIError("forbidden");
  const signal = AbortSignal.timeout(10_000);
  return async (path: string, query: Record<string, string> = {}) => {
    try {
      const url = new URL(path, "https://api.vercel.com");
      url.search = new URLSearchParams({ teamId: teamID, ...query }).toString();
      const response = await fetcher(url, {
        method: "GET",
        headers: {
          Authorization: `Bearer ${token}`,
          Accept: "application/json",
        },
        cache: "no-store",
        redirect: "error",
        signal,
      });
      if (
        response.redirected ||
        (response.url && new URL(response.url).origin !== url.origin)
      )
        throw unavailable();
      if (!response.ok)
        throw new VercelAPIError(
          response.status === 401
            ? "disconnected"
            : response.status === 403
              ? "forbidden"
              : response.status === 404
                ? "not_found"
                : "unavailable",
        );
      return await readJSON(response);
    } catch (error) {
      if (error instanceof VercelAPIError) throw error;
      throw unavailable();
    }
  };
}

export async function fetchVercelProject({
  token,
  teamID,
  projectID,
  fetcher = fetch,
}: {
  token: string;
  teamID: string;
  projectID: string;
  fetcher?: typeof fetch;
}): Promise<VercelProjectSnapshot> {
  const get = vercelReader({ token, teamID, fetcher });
  if (
    typeof projectID !== "string" ||
    !/^prj_[A-Za-z0-9]{1,128}$/.test(projectID)
  )
    throw new VercelAPIError("not_found");
  try {
    const project = record(await get(`/v9/projects/${projectID}`));
    const name = text(project.name, 200);
    if (
      typeof project.id !== "string" ||
      typeof project.accountId !== "string" ||
      !name
    )
      throw unavailable();
    if (project.id !== projectID || project.accountId !== teamID)
      throw new VercelAPIError("forbidden");
    const domains: VercelDomain[] = [];
    const readDomains = async () => {
      let until: number | undefined;
      const visited = new Set<number>();
      for (let page = 0; page < 10; page++) {
        const result = record(
          await get(`/v9/projects/${projectID}/domains`, {
            limit: "100",
            ...(until === undefined ? {} : { until: String(until) }),
          }),
        );
        if (!Array.isArray(result.domains) || result.domains.length > 100)
          throw unavailable();
        for (const row of result.domains) {
          const value = record(row),
            name = domain(value.name, true);
          if (!name) throw unavailable();
          if (value.projectId !== undefined && value.projectId !== projectID)
            throw new VercelAPIError("forbidden");
          domains.push({
            name,
            verified: value.verified === true,
            redirect: domain(value.redirect),
          });
        }
        const next =
          result.pagination == null ? null : record(result.pagination).next;
        if (next == null) return;
        if (
          typeof next !== "number" ||
          !Number.isSafeInteger(next) ||
          next <= 0 ||
          visited.has(next)
        )
          throw unavailable();
        visited.add(next);
        until = next;
      }
      throw unavailable(); // Do not silently present a truncated domain inventory.
    };
    const [latestRaw] = await Promise.all([
      get("/v7/deployments", { projectId: projectID, limit: "5" }),
      readDomains(),
    ]);
    const latest = record(latestRaw);
    if (!Array.isArray(latest.deployments) || latest.deployments.length > 100)
      throw unavailable();
    const deployments = latest.deployments
      .map((value) => deployment(value, projectID))
      .sort((a, b) => (b.createdAt || "").localeCompare(a.createdAt || ""))
      .slice(0, 5);
    const production =
      project.targets == null ? undefined : record(project.targets).production;
    return {
      projectID,
      name,
      framework: text(project.framework, 100),
      nodeVersion: text(project.nodeVersion, 32),
      checkedAt: new Date().toISOString(),
      ...(production == null
        ? {}
        : { productionDeployment: deployment(production, projectID) }),
      deployments,
      domains: [
        ...new Map(domains.map((value) => [value.name, value])).values(),
      ],
    };
  } catch (error) {
    if (error instanceof VercelAPIError) throw error;
    // Never propagate upstream body text, fetch errors, URLs, or bearer credentials.
    throw unavailable();
  }
}

// The documented v10 API uses timestamp cursors or Base32 continuation tokens.
function projectCursor(value: unknown): string {
  if (typeof value === "number" && Number.isSafeInteger(value) && value > 0)
    return String(value);
  if (typeof value === "string" && value.length <= 4096) {
    if (/^[1-9][0-9]{0,15}$/.test(value) && Number.isSafeInteger(Number(value)))
      return value;
    if (/^[A-Z2-7]+={0,6}$/i.test(value)) return value;
  }
  throw unavailable();
}

export type VercelProjectList = {
  projects: { id: string; name: string }[];
  nextCursor: string | null;
};

/** One bounded page for the project picker; callers explicitly request subsequent pages. */
export async function listVercelProjects({
  token,
  teamID,
  cursor,
  fetcher,
}: VercelReadOptions & { cursor?: string }): Promise<VercelProjectList> {
  const get = vercelReader({ token, teamID, fetcher });
  if (
    cursor !== undefined &&
    (typeof cursor !== "string" || projectCursor(cursor) !== cursor)
  )
    throw unavailable();
  const raw = await get("/v10/projects", {
    limit: "100",
    ...(cursor === undefined ? {} : { from: cursor }),
  });
  const result = Array.isArray(raw)
    ? { projects: raw, pagination: null }
    : record(raw);
  if (!Array.isArray(result.projects) || result.projects.length > 100)
    throw unavailable();
  const projects = result.projects.map((value) => {
    const project = record(value),
      name = text(project.name, 200);
    if (
      typeof project.id !== "string" ||
      !/^prj_[A-Za-z0-9]{1,128}$/.test(project.id) ||
      typeof project.accountId !== "string" ||
      !name
    )
      throw unavailable();
    if (project.accountId !== teamID) throw new VercelAPIError("forbidden");
    return { id: project.id, name };
  });
  const next =
    result.pagination == null ? null : record(result.pagination).next;
  const nextCursor = next == null ? null : projectCursor(next);
  if (nextCursor !== null && nextCursor === cursor) throw unavailable();
  return { projects, nextCursor };
}

/** Only the operator-authorized deletion service may call this server-side adapter.
 * previouslyVerifiedTarget must come from its persisted verified mapping, never request input.
 * https://vercel.com/docs/rest-api/projects/delete-a-project
 */
export async function deleteVercelProject({
  token,
  teamID,
  projectID,
  fetcher = fetch,
  previouslyVerifiedTarget,
}: VercelReadOptions & {
  projectID: string;
  previouslyVerifiedTarget?: { projectID: string; teamID: string };
}): Promise<{ status: "deleted" | "already_absent" }> {
  const get = vercelReader({ token, teamID, fetcher });
  if (
    typeof projectID !== "string" ||
    !/^prj_[A-Za-z0-9]{1,128}$/.test(projectID)
  )
    throw new VercelAPIError("not_found");
  if (
    previouslyVerifiedTarget !== undefined &&
    (!previouslyVerifiedTarget ||
      previouslyVerifiedTarget.projectID !== projectID ||
      previouslyVerifiedTarget.teamID !== teamID)
  )
    throw new VercelAPIError("forbidden");
  try {
    const project = record(await get(`/v9/projects/${projectID}`));
    if (project.id !== projectID || project.accountId !== teamID)
      throw new VercelAPIError("forbidden");
    const url = new URL(`/v9/projects/${projectID}`, "https://api.vercel.com");
    url.search = new URLSearchParams({ teamId: teamID }).toString();
    const response = await fetcher(url, {
      method: "DELETE",
      headers: { Authorization: `Bearer ${token}`, Accept: "application/json" },
      cache: "no-store",
      redirect: "error",
      signal: AbortSignal.timeout(10_000),
    });
    if (
      response.redirected ||
      (response.url && new URL(response.url).origin !== url.origin)
    )
      throw unavailable();
    if (response.status === 204) return { status: "deleted" };
    throw new VercelAPIError(
      response.status === 401
        ? "disconnected"
        : response.status === 403
          ? "deletion_forbidden"
          : response.status === 404
            ? "not_found"
            : "unavailable",
    );
  } catch (error) {
    if (error instanceof VercelAPIError) {
      if (error.code === "not_found" && previouslyVerifiedTarget)
        return { status: "already_absent" };
      throw error;
    }
    throw unavailable();
  }
}
