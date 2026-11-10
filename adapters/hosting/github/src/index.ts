import { gitRootDirectory } from "@spitzli/hosting-contracts";
type Fetch = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;
import type {
  GitHubConfig,
  GitRepositoryBinding,
  GitSource,
  GitHubWebhook,
  GitHubProvider,
} from "@spitzli/hosting-provider-contracts/github";
export type {
  GitHubConfig,
  GitRepositoryBinding,
  GitSource,
  GitHubWebhook,
} from "@spitzli/hosting-provider-contracts/github";
import { createHash, createHmac, sign, timingSafeEqual } from "node:crypto";
import { HostingError } from "@spitzli/hosting-contracts";
const denied = () =>
  new HostingError(
    403,
    "GitHub access could not be verified. Check installation and repository permissions.",
  );
const unavailable = () =>
  new HostingError(
    502,
    "GitHub could not complete this request. Check the current state before retrying.",
  );
function id(value: unknown): string {
  if (
    (typeof value !== "string" && typeof value !== "number") ||
    !/^[1-9][0-9]{0,15}$/.test(String(value)) ||
    !Number.isSafeInteger(Number(value))
  )
    throw denied();
  return String(value);
}
function name(value: unknown): string {
  if (
    typeof value !== "string" ||
    !/^[A-Za-z0-9_.-]{1,100}$/.test(value) ||
    value === "." ||
    value === ".."
  )
    throw denied();
  return value;
}
function branchName(value: unknown): string {
  if (
    typeof value !== "string" ||
    value.length > 255 ||
    !value ||
    /[\x00-\x20\x7f~^:?*\[\\]/.test(value) ||
    value.includes("..") ||
    value.includes("@{") ||
    value.startsWith("/") ||
    value.endsWith("/") ||
    value.endsWith(".") ||
    value.split("/").some((v) => !v || v.startsWith(".") || v.endsWith(".lock"))
  )
    throw denied();
  return value;
}
function sha(value: unknown): string {
  if (typeof value !== "string" || !/^[0-9a-f]{40}$/.test(value))
    throw denied();
  return value;
}
async function readBounded(
  response: Response,
  max = 2_000_000,
): Promise<Buffer> {
  if (Number(response.headers.get("content-length")) > max) {
    await response.body?.cancel();
    throw unavailable();
  }
  const reader = response.body?.getReader();
  if (!reader) throw unavailable();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.length;
      if (size > max) {
        await reader.cancel();
        throw unavailable();
      }
      chunks.push(value);
    }
    return Buffer.concat(chunks);
  } catch {
    throw unavailable();
  } finally {
    reader.releaseLock();
  }
}
export function createGitHubProvider(
  config: GitHubConfig,
  fetchImpl: Fetch = fetch,
  options: { allowLoopback?: boolean } = {},
): GitHubProvider {
  function jwt() {
    if (!config.appID || !config.privateKey)
      throw new HostingError(503, "GitHub deployments are not configured yet.");
    const now = Math.floor(Date.now() / 1000);
    const data =
      Buffer.from(JSON.stringify({ alg: "RS256", typ: "JWT" })).toString(
        "base64url",
      ) +
      "." +
      Buffer.from(
        JSON.stringify({ iat: now - 60, exp: now + 540, iss: config.appID }),
      ).toString("base64url");
    try {
      return (
        data +
        "." +
        sign("RSA-SHA256", Buffer.from(data), config.privateKey).toString(
          "base64url",
        )
      );
    } catch {
      throw new HostingError(503, "GitHub deployments are not configured yet.");
    }
  }
  async function request(
    url: string,
    token?: string,
    init: RequestInit = {},
    manual = false,
  ): Promise<Response> {
    const endpoint = new URL(url);
    if (
      ![
        "https://api.github.com",
        "https://github.com",
        "https://codeload.github.com",
      ].includes(endpoint.origin) ||
      endpoint.username ||
      endpoint.password
    )
      throw denied();
    if (token && endpoint.origin === "https://codeload.github.com")
      throw denied();
    try {
      return await fetchImpl(endpoint, {
        ...init,
        headers: {
          Accept: "application/vnd.github+json",
          "X-GitHub-Api-Version": "2022-11-28",
          ...(token ? { Authorization: "Bearer " + token } : {}),
          ...init.headers,
        },
        cache: "no-store",
        redirect: manual ? "manual" : "error",
        signal: AbortSignal.timeout(30_000),
      });
    } catch {
      throw unavailable();
    }
  }
  async function json(path: string, token: string, init: RequestInit = {}) {
    try {
      const r = await request("https://api.github.com" + path, token, init);
      if (!r.ok) {
        await r.body?.cancel();
        throw [401, 403, 404].includes(r.status) ? denied() : unavailable();
      }
      return JSON.parse((await readBounded(r)).toString());
    } catch (e) {
      if (e instanceof HostingError) throw e;
      throw unavailable();
    }
  }
  async function pages(path: string, token: string, key: string) {
    const result: {
      id?: unknown;
      app_id?: unknown;
      suspended_at?: unknown;
      account?: { id?: unknown; login?: unknown };
      owner?: { login?: unknown };
      name?: unknown;
      default_branch?: unknown;
      permissions?: { pull?: unknown };
      expired?: unknown;
      digest?: unknown;
      size_in_bytes?: unknown;
      created_at?: unknown;
      workflow_run?: {
        id?: unknown;
        repository_id?: unknown;
        head_repository_id?: unknown;
        head_sha?: unknown;
      };
    }[] = [];
    for (let page = 1; page <= 20; page++) {
      const r = await json(path + "?per_page=100&page=" + page, token);
      if (!Array.isArray(r[key])) throw denied();
      result.push(...r[key]);
      if (r[key].length < 100) return result;
    }
    throw new HostingError(
      422,
      "GitHub repository selection exceeds the supported limit.",
    );
  }
  async function installation(installationID: string) {
    const r = await json("/app/installations/" + id(installationID), jwt());
    if (
      id(r.id) !== installationID ||
      String(r.app_id) !== config.appID ||
      r.suspended_at ||
      !r.account ||
      !["read", "write"].includes(r.permissions?.contents) ||
      r.permissions?.checks !== "write" ||
      !["read", "write"].includes(r.permissions?.metadata)
    )
      throw denied();
    return r;
  }
  async function repositoryToken(
    installationID: string,
    repositoryID: string,
    checks: boolean | "actions" = false,
  ) {
    const installed = await installation(installationID);
    if (
      checks === "actions" &&
      !["read", "write"].includes(installed.permissions?.actions)
    )
      throw denied();
    const permissions =
      checks === "actions"
        ? { actions: "read", metadata: "read" }
        : checks
          ? { checks: "write", metadata: "read" }
          : { contents: "read", metadata: "read" };
    const r = await json(
      "/app/installations/" + id(installationID) + "/access_tokens",
      jwt(),
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          repository_ids: [Number(id(repositoryID))],
          permissions,
        }),
      },
    );
    if (
      typeof r.token !== "string" ||
      r.token.length < 1 ||
      r.token.length > 8192 ||
      !Number.isFinite(Date.parse(r.expires_at)) ||
      Date.parse(r.expires_at) <= Date.now() ||
      !Array.isArray(r.repositories) ||
      r.repositories.length !== 1 ||
      id(r.repositories[0]?.id) !== repositoryID ||
      Object.entries(permissions).some(([k, v]) => r.permissions?.[k] !== v) ||
      Object.keys(r.permissions ?? {}).some((k) => !(k in permissions))
    )
      throw denied();
    return r.token;
  }
  function repository(r: {
    id?: unknown;
    owner?: { login?: unknown };
    name?: unknown;
    default_branch?: unknown;
  }) {
    return {
      repositoryID: id(r.id),
      owner: name(r.owner?.login),
      name: name(r.name),
      fullName: name(r.owner?.login) + "/" + name(r.name),
      defaultBranch: branchName(r.default_branch),
    };
  }
  async function listUserInstallations(userToken: string) {
    return (await pages("/user/installations", userToken, "installations"))
      .filter((r) => String(r.app_id) === config.appID && !r.suspended_at)
      .map((r) => ({
        installationID: id(r.id),
        accountID: id(r.account?.id),
        accountLogin: name(r.account?.login),
      }));
  }
  async function listUserRepositories(
    userToken: string,
    installationID: string,
  ) {
    if (
      !(await listUserInstallations(userToken)).some(
        (i) => i.installationID === id(installationID),
      )
    )
      throw denied();
    await installation(installationID);
    return (
      await pages(
        "/user/installations/" + id(installationID) + "/repositories",
        userToken,
        "repositories",
      )
    )
      .filter((r) => r.permissions?.pull === true)
      .map(repository);
  }
  async function revalidateRepository(
    installationID: string,
    repositoryID: string,
  ) {
    const token = await repositoryToken(installationID, repositoryID);
    const r = repository(
      await json("/repositories/" + id(repositoryID), token),
    );
    if (r.repositoryID !== repositoryID) throw denied();
    return r;
  }
  async function verifyRepository(
    userToken: string,
    installationID: string,
    repositoryID: string,
  ): Promise<GitRepositoryBinding> {
    const repos = await listUserRepositories(userToken, installationID);
    if (!repos.some((r) => r.repositoryID === id(repositoryID))) throw denied();
    const i = await installation(installationID);
    const r = await revalidateRepository(installationID, repositoryID);
    return {
      ...r,
      installationID,
      accountID: id(i.account.id),
      accountLogin: name(i.account.login),
      permissions: {
        metadata: i.permissions.metadata,
        contents: i.permissions.contents,
        checks: i.permissions.checks,
        ...(["read", "write"].includes(i.permissions.actions)
          ? { actions: i.permissions.actions }
          : {}),
      },
    };
  }
  async function resolveSource(source: GitSource) {
    const token = await repositoryToken(
      source.installationID,
      source.repositoryID,
    );
    const r = repository(
      await json("/repositories/" + id(source.repositoryID), token),
    );
    if (r.repositoryID !== source.repositoryID) throw denied();
    const branch = branchName(source.branch);
    const head = await json(
      "/repos/" + r.fullName + "/branches/" + encodeURIComponent(branch),
      token,
    );
    if (head.name !== branch) throw denied();
    return {
      sha: sha(head.commit?.sha),
      repositoryID: r.repositoryID,
      repositoryName: r.fullName,
      branch,
    };
  }
  async function resolveCommit(source: GitSource, commit: string) {
    const exact = sha(commit),
      token = await repositoryToken(source.installationID, source.repositoryID);
    const r = repository(
      await json("/repositories/" + id(source.repositoryID), token),
    );
    if (r.repositoryID !== source.repositoryID) throw denied();
    const resolved = await json(
      "/repos/" + r.fullName + "/commits/" + exact,
      token,
    );
    if (sha(resolved.sha) !== exact) throw denied();
    return {
      sha: exact,
      repositoryID: r.repositoryID,
      repositoryName: r.fullName,
      branch: branchName(source.branch),
    };
  }
  async function readSourceFile(
    source: GitSource,
    commit: string,
    path: string,
  ): Promise<string | null> {
    const exact = sha(commit);
    gitRootDirectory.parse(path);
    const token = await repositoryToken(
      source.installationID,
      source.repositoryID,
    );
    const r = repository(
      await json("/repositories/" + id(source.repositoryID), token),
    );
    if (r.repositoryID !== source.repositoryID) throw denied();
    const response = await request(
      "https://api.github.com/repos/" +
        r.fullName +
        "/contents/" +
        path.split("/").map(encodeURIComponent).join("/") +
        "?ref=" +
        exact,
      token,
    );
    if (response.status === 404) {
      await response.body?.cancel();
      return null;
    }
    if (!response.ok) {
      await response.body?.cancel();
      throw denied();
    }
    const file = JSON.parse((await readBounded(response, 100000)).toString());
    if (
      file.type !== "file" ||
      file.encoding !== "base64" ||
      !Number.isInteger(file.size) ||
      file.size < 0 ||
      file.size > 65536 ||
      typeof file.content !== "string"
    )
      throw unavailable();
    const encoded = file.content.replace(/\s/g, "");
    if (!/^[A-Za-z0-9+/]*={0,2}$/.test(encoded)) throw unavailable();
    const decoded = Buffer.from(encoded, "base64");
    if (decoded.length !== file.size) throw unavailable();
    return decoded.toString("utf8");
  }
  async function exchangeOAuthCode(code: string, redirectURI: string) {
    if (!config.clientID || !config.clientSecret)
      throw new HostingError(503, "GitHub deployments are not configured yet.");
    let callback: URL;
    try {
      callback = new URL(redirectURI);
    } catch {
      throw denied();
    }
    const local =
      options.allowLoopback === true &&
      callback.protocol === "http:" &&
      ["localhost", "127.0.0.1"].includes(callback.hostname);
    if (
      !code ||
      code.length > 1024 ||
      (!local && callback.protocol !== "https:") ||
      callback.username ||
      callback.password ||
      callback.hash
    )
      throw denied();
    const r = await request(
      "https://github.com/login/oauth/access_token",
      undefined,
      {
        method: "POST",
        headers: {
          Accept: "application/json",
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          client_id: config.clientID,
          client_secret: config.clientSecret,
          code,
          redirect_uri: redirectURI,
        }),
      },
    );
    try {
      if (!r.ok) throw denied();
      const value = JSON.parse((await readBounded(r)).toString());
      if (
        typeof value.access_token !== "string" ||
        !value.access_token ||
        value.access_token.length > 8192 ||
        value.token_type?.toLowerCase() !== "bearer"
      )
        throw denied();
      return value.access_token as string;
    } catch (e) {
      if (e instanceof HostingError) throw e;
      throw unavailable();
    }
  }
  async function prepareSourceDownload(
    installationID: string,
    repositoryID: string,
    revision: string,
  ) {
    const pinned = sha(revision),
      token = await repositoryToken(installationID, repositoryID);
    const r = repository(
      await json("/repositories/" + id(repositoryID), token),
    );
    if (r.repositoryID !== repositoryID) throw denied();
    // The signed download URL is trusted-service-only; never forward installation credentials.
    const response = await request(
      "https://api.github.com/repos/" + r.fullName + "/tarball/" + pinned,
      token,
      {},
      true,
    );
    if (response.status !== 302) {
      await response.body?.cancel();
      throw unavailable();
    }
    const location = response.headers.get("location");
    await response.body?.cancel();
    if (!location) throw unavailable();
    let url: URL;
    try {
      url = new URL(location);
    } catch {
      throw denied();
    }
    if (
      url.origin !== "https://codeload.github.com" ||
      url.username ||
      url.password
    )
      throw denied();
    return {
      url: url.toString(),
      sha: pinned,
      repositoryID,
      repositoryName: r.fullName,
    };
  }
  async function fetchSource(
    installationID: string,
    repositoryID: string,
    revision: string,
  ) {
    const descriptor = await prepareSourceDownload(
      installationID,
      repositoryID,
      revision,
    );
    const archive = await request(descriptor.url);
    if (!archive.ok) {
      await archive.body?.cancel();
      throw unavailable();
    }
    const bytes = await readBounded(archive, 100_000_000);
    // Archive extraction and expanded-size/path/link validation belong to the trusted worker boundary.
    return {
      bytes,
      sha: descriptor.sha,
      repositoryID,
      repositoryName: descriptor.repositoryName,
      checksum: createHash("sha256").update(bytes).digest("hex"),
    };
  }
  async function verifyActionsRun(input: {
    installationID: string;
    repositoryID: string;
    workflowPath: string;
    runID: string;
    runAttempt: number;
    branch: string;
    sha: string;
    artifactName: string;
  }) {
    if (
      input.runAttempt !== 1 ||
      !/^\.github\/workflows\/[A-Za-z0-9_-]+\.ya?ml$/.test(
        input.workflowPath,
      ) ||
      !/^[A-Za-z0-9_-]{1,160}$/.test(input.artifactName)
    )
      throw denied();
    const revision = sha(input.sha),
      runID = id(input.runID),
      branch = branchName(input.branch);
    const token = await repositoryToken(
      input.installationID,
      input.repositoryID,
      "actions",
    );
    const repo = repository(
      await json("/repositories/" + id(input.repositoryID), token),
    );
    if (repo.repositoryID !== input.repositoryID) throw denied();
    const base = "/repos/" + repo.fullName + "/actions";
    const workflow = await json(
      base +
        "/workflows/" +
        encodeURIComponent(input.workflowPath.split("/").at(-1)!),
      token,
    );
    if (workflow.path !== input.workflowPath || workflow.state !== "active")
      throw denied();
    const run = await json(base + "/runs/" + runID + "/attempts/1", token);
    if (
      id(run.id) !== runID ||
      run.run_attempt !== 1 ||
      id(run.workflow_id) !== id(workflow.id) ||
      run.path !== input.workflowPath ||
      id(run.repository?.id) !== input.repositoryID ||
      id(run.head_repository?.id) !== input.repositoryID ||
      run.head_branch !== branch ||
      run.head_sha !== revision ||
      !["push", "workflow_dispatch"].includes(run.event) ||
      run.status !== "completed" ||
      run.conclusion !== "success" ||
      typeof run.created_at !== "string" ||
      !Number.isFinite(Date.parse(run.created_at))
    )
      throw denied();
    // Attempt-specific execution is accepted only before any rerun can replace its artifacts.
    const current = await json(base + "/runs/" + runID, token);
    if (
      current.run_attempt !== 1 ||
      current.head_sha !== revision ||
      current.conclusion !== "success"
    )
      throw denied();
    const artifacts = await pages(
      base + "/runs/" + runID + "/artifacts",
      token,
      "artifacts",
    );
    const matches = artifacts.filter((a) => a.name === input.artifactName);
    if (matches.length !== 1) throw denied();
    const artifact = matches[0];
    if (
      artifact.expired !== false ||
      typeof artifact.digest !== "string" ||
      !/^sha256:[a-f0-9]{64}$/.test(artifact.digest) ||
      typeof artifact.size_in_bytes !== "number" ||
      !Number.isSafeInteger(artifact.size_in_bytes) ||
      artifact.size_in_bytes < 1 ||
      artifact.size_in_bytes > 600_000_000 ||
      id(artifact.workflow_run?.id) !== runID ||
      id(artifact.workflow_run?.repository_id) !== input.repositoryID ||
      id(artifact.workflow_run?.head_repository_id) !== input.repositoryID ||
      artifact.workflow_run?.head_sha !== revision ||
      typeof artifact.created_at !== "string" ||
      Date.parse(artifact.created_at) < Date.parse(run.created_at) ||
      !Number.isFinite(Date.parse(artifact.created_at))
    )
      throw denied();
    const response = await request(
      "https://api.github.com" +
        base +
        "/artifacts/" +
        id(artifact.id) +
        "/zip",
      token,
      {},
      true,
    );
    const location = response.headers.get("location");
    await response.body?.cancel();
    if (response.status !== 302 || !location) throw unavailable();
    const url = new URL(location);
    if (
      url.protocol !== "https:" ||
      url.username ||
      url.password ||
      url.port ||
      !(
        url.hostname.endsWith(".blob.core.windows.net") ||
        url.hostname.endsWith(".actions.githubusercontent.com")
      )
    )
      throw denied();
    return {
      runID,
      runAttempt: 1,
      workflowID: id(workflow.id),
      workflowPath: input.workflowPath,
      repositoryID: input.repositoryID,
      sha: revision,
      branch,
      createdAt: run.created_at,
      artifact: {
        id: id(artifact.id),
        name: input.artifactName,
        sizeBytes: artifact.size_in_bytes,
        digest: artifact.digest,
        downloadURL: url.toString(),
        repository: repo.fullName,
        ref: "refs/heads/" + branch,
        runID,
        runAttempt: 1,
        workflowPath: input.workflowPath,
        repositoryID: input.repositoryID,
      },
    };
  }
  async function reportCheck(input: {
    installationID: string;
    repositoryID: string;
    sha: string;
    externalID: string;
    checkID?: string;
    status: "queued" | "in_progress" | "completed";
    conclusion?: "success" | "failure" | "cancelled" | "neutral";
  }) {
    if (
      !/^[A-Za-z0-9:_-]{1,100}$/.test(input.externalID) ||
      !["queued", "in_progress", "completed"].includes(input.status) ||
      (input.status === "completed"
        ? !["success", "failure", "cancelled", "neutral"].includes(
            input.conclusion ?? "",
          )
        : input.conclusion !== undefined)
    )
      throw denied();
    const token = await repositoryToken(
      input.installationID,
      input.repositoryID,
      true,
    );
    const r = repository(
      await json("/repositories/" + id(input.repositoryID), token),
    );
    if (r.repositoryID !== input.repositoryID) throw denied();
    const base = "/repos/" + r.fullName + "/check-runs";
    let checkID = input.checkID;
    if (!checkID) {
      let complete = false;
      const matches = new Set<string>();
      for (let page = 1; page <= 10; page++) {
        const existing = await json(
          "/repos/" +
            r.fullName +
            "/commits/" +
            sha(input.sha) +
            "/check-runs?check_name=Webdock%20deployment&filter=all&per_page=100&page=" +
            page,
          token,
        );
        if (!Array.isArray(existing.check_runs)) throw denied();
        for (const check of existing.check_runs)
          if (
            check.external_id === input.externalID &&
            check.head_sha === input.sha &&
            String(check.app?.id) === config.appID
          )
            matches.add(id(check.id));
        if (matches.size > 1)
          throw new HostingError(
            409,
            "Multiple GitHub checks match this build. Reconciliation is required.",
          );
        if (existing.check_runs.length < 100) {
          complete = true;
          break;
        }
      }
      if (!complete)
        throw new HostingError(
          409,
          "GitHub check reconciliation exceeds the supported limit.",
        );
      checkID = [...matches][0];
    }
    const payload = {
      name: "Webdock deployment",
      ...(checkID ? {} : { head_sha: sha(input.sha) }),
      external_id: input.externalID,
      status: input.status,
      ...(input.conclusion
        ? {
            conclusion: input.conclusion,
            completed_at: new Date().toISOString(),
          }
        : {}),
    };
    if (checkID) {
      const current = await json(base + "/" + id(checkID), token);
      if (
        String(current.id) !== checkID ||
        current.external_id !== input.externalID ||
        current.head_sha !== input.sha ||
        String(current.app?.id) !== config.appID
      )
        throw denied();
    }
    const result = await json(
      base + (checkID ? "/" + id(checkID) : ""),
      token,
      {
        method: checkID ? "PATCH" : "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      },
    );
    return { checkID: id(result.id) };
  }
  return {
    listUserInstallations,
    listUserRepositories,
    verifyRepository,
    revalidateRepository,
    resolveSource,
    resolveCommit,
    readSourceFile,
    exchangeOAuthCode,
    fetchSource,
    prepareSourceDownload,
    reportCheck,
    verifyActionsRun,
  };
}
export function parseGitHubWebhook(
  rawBody: Uint8Array,
  headers: Headers,
  secret: string,
): GitHubWebhook {
  if (!secret)
    throw new HostingError(503, "GitHub webhooks are not configured yet.");
  if (rawBody.byteLength > 2_000_000)
    throw new HostingError(413, "GitHub event is too large.");
  const signature = headers.get("x-hub-signature-256") ?? "";
  if (
    !/^sha256=[0-9a-f]{64}$/.test(signature) ||
    !timingSafeEqual(
      Buffer.from(signature.slice(7), "hex"),
      createHmac("sha256", secret).update(rawBody).digest(),
    )
  )
    throw denied();
  const deliveryID = headers.get("x-github-delivery") ?? "";
  if (!/^[A-Za-z0-9-]{1,100}$/.test(deliveryID)) throw denied();
  let payload: {
    installation?: { id?: unknown };
    repository?: { id?: unknown };
    ref?: unknown;
    deleted?: unknown;
    after?: unknown;
    action?: unknown;
    workflow_run?: {
      run_attempt?: unknown;
      path?: unknown;
      conclusion?: unknown;
      head_repository?: { id?: unknown };
      head_branch?: unknown;
      head_sha?: unknown;
      id?: unknown;
    };
    repositories_added?: { id?: unknown }[];
    repositories_removed?: { id?: unknown }[];
  };
  try {
    payload = JSON.parse(Buffer.from(rawBody).toString());
  } catch {
    throw denied();
  }
  if (!payload || typeof payload !== "object") throw denied();
  const event = headers.get("x-github-event");
  const installationID = id(payload.installation?.id);
  if (event === "push") {
    if (
      typeof payload.ref !== "string" ||
      !payload.ref.startsWith("refs/heads/") ||
      typeof payload.deleted !== "boolean"
    )
      throw denied();
    return {
      deliveryID,
      event,
      installationID,
      repositoryID: id(payload.repository?.id),
      branch: branchName(payload.ref.slice(11)),
      sha: sha(payload.after),
      deleted: payload.deleted,
    };
  }
  if (event === "workflow_run") {
    const run = payload.workflow_run;
    if (
      payload.action !== "completed" ||
      !run ||
      typeof run.run_attempt !== "number" ||
      !Number.isSafeInteger(run.run_attempt) ||
      run.run_attempt < 1 ||
      typeof run.path !== "string" ||
      !/^\.github\/workflows\/[A-Za-z0-9_-]+\.ya?ml$/.test(run.path) ||
      typeof run.conclusion !== "string" ||
      run.conclusion.length > 40 ||
      id(run.head_repository?.id) !== id(payload.repository?.id)
    )
      throw denied();
    return {
      deliveryID,
      event,
      installationID,
      repositoryID: id(payload.repository?.id),
      branch: branchName(run.head_branch),
      sha: sha(run.head_sha),
      runID: id(run.id),
      runAttempt: run.run_attempt,
      workflowPath: run.path,
      conclusion: run.conclusion,
      action: payload.action,
    };
  }
  const actions: Record<string, string[]> = {
    installation: [
      "created",
      "deleted",
      "suspend",
      "unsuspend",
      "new_permissions_accepted",
    ],
    installation_repositories: ["added", "removed"],
    repository: [
      "deleted",
      "archived",
      "unarchived",
      "renamed",
      "transferred",
      "privatized",
      "publicized",
      "edited",
    ],
  };
  if (
    !event ||
    typeof payload.action !== "string" ||
    !actions[event]?.includes(payload.action)
  )
    throw denied();
  const repositoryIDs =
    event === "repository"
      ? [id(payload.repository?.id)]
      : [
          ...(payload.repositories_added ?? []),
          ...(payload.repositories_removed ?? []),
        ].map((r) => id(r.id));
  return {
    deliveryID,
    event: event as "installation" | "installation_repositories" | "repository",
    installationID,
    action: payload.action,
    repositoryIDs,
  };
}
