import type { HostingPorts } from "./ports";
import type { Fetch } from "./ports";
import { HostingError, gitHealthPath } from "@spitzli/hosting-contracts";
import type { GitVercelCredential } from "./git-vercel";
export type Dependencies = {
  vercelJSON: ReturnType<
    typeof import("./byok-vercel").createHostingByokVercel
  >["vercelJSON"];
};
type ProviderJSON = (
  path: string,
  token: string,
  teamID: string,
) => Promise<Record<string, unknown>>;
export function createHostingGitVercelPublication(dependencies: Dependencies) {
  const defaultJSON: ProviderJSON = async (path, token, teamID) =>
    dependencies.vercelJSON(path, token, teamID);
  const denied = () =>
    new HostingError(403, "Vercel deployment identity could not be verified.");
  function releaseID(value: string) {
    if (!/^[1-9][0-9]{0,19}$/.test(value)) throw denied();
    return value;
  }
  function deploymentID(value: unknown) {
    if (typeof value !== "string" || !/^dpl_[A-Za-z0-9]+$/.test(value))
      throw denied();
    return value;
  }
  function deploymentURL(value: unknown) {
    if (typeof value !== "string") throw denied();
    let url: URL;
    try {
      url = new URL(value.includes("://") ? value : "https://" + value);
    } catch {
      throw denied();
    }
    if (
      url.protocol !== "https:" ||
      !/^[a-z0-9][a-z0-9-]*\.vercel\.app$/.test(url.hostname) ||
      url.username ||
      url.password ||
      url.port ||
      url.pathname !== "/" ||
      url.search ||
      url.hash
    )
      throw denied();
    return url.origin;
  }
  function euRegions(value: unknown) {
    return Array.isArray(value) && value.length === 1 && value[0] === "fra1";
  }
  function createGitVercelObserver(
    credential: GitVercelCredential,
    json: ProviderJSON = defaultJSON,
    fetchImpl: Fetch = fetch,
  ) {
    if (
      !/^team_[A-Za-z0-9]+$/.test(credential.teamID) ||
      !/^prj_[A-Za-z0-9]+$/.test(credential.projectID)
    )
      throw denied();
    function verified(row: Record<string, unknown>, expected: string) {
      if (
        !row ||
        row.ownerId !== credential.teamID ||
        row.projectId !== credential.projectID ||
        (row.meta as Record<string, unknown> | undefined)?.webdockReleaseID !==
          releaseID(expected) ||
        row.prebuilt !== true ||
        row.target !== "production" ||
        row.deletedAt
      )
        throw denied();
      return {
        deploymentID: deploymentID(row.id),
        url: deploymentURL(row.url),
      };
    }
    async function resolveDeployment(input: {
      providerURL: string;
      releaseID: string;
    }) {
      const origin = deploymentURL(input.providerURL);
      const row = await json(
        "/v13/deployments/" + encodeURIComponent(new URL(origin).hostname),
        credential.token,
        credential.teamID,
      );
      const result = verified(row, input.releaseID);
      if (result.url !== origin) throw denied();
      return result;
    }
    async function reconcileDeployment(input: {
      releaseID: string;
      since: number;
    }): Promise<
      | {
          status: "not-found" | "ambiguous";
        }
      | {
          status: "found";
          deploymentID: string;
          url: string;
        }
    > {
      releaseID(input.releaseID);
      if (!Number.isSafeInteger(input.since) || input.since < 0) throw denied();
      let until: number | undefined;
      const candidates = new Set<string>();
      let complete = false;
      for (let page = 0; page < 10; page++) {
        const params = new URLSearchParams({
          projectId: credential.projectID,
          since: String(input.since),
          limit: "100",
          target: "production",
        });
        if (until !== undefined) params.set("until", String(until));
        const response = await json(
          "/v6/deployments?" + params,
          credential.token,
          credential.teamID,
        );
        if (!Array.isArray(response.deployments)) throw denied();
        for (const row of response.deployments)
          if (
            row.projectId === credential.projectID &&
            (row.meta as Record<string, unknown> | undefined)
              ?.webdockReleaseID === input.releaseID
          )
            candidates.add(deploymentID(row.uid ?? row.id));
        if (candidates.size > 1) return { status: "ambiguous" };
        const next = (
          response.pagination as Record<string, unknown> | undefined
        )?.next;
        if (next === null || next === undefined) {
          complete = true;
          break;
        }
        if (
          typeof next !== "number" ||
          !Number.isSafeInteger(next) ||
          next < 0 ||
          (until !== undefined && next >= until)
        )
          return { status: "ambiguous" };
        until = next;
      }
      if (!complete) return { status: "ambiguous" };
      if (!candidates.size) return { status: "not-found" };
      const candidate = [...candidates][0];
      const row = await json(
        "/v13/deployments/" + candidate,
        credential.token,
        credential.teamID,
      );
      const result = verified(row, input.releaseID);
      if (result.deploymentID !== candidate) throw denied();
      return { status: "found", ...result };
    }
    async function observeDeployment(input: {
      deploymentID: string;
      releaseID: string;
      healthPath: string;
    }) {
      if (
        !input.healthPath.startsWith("/") ||
        input.healthPath.startsWith("//") ||
        input.healthPath.includes("\\") ||
        /[\x00-\x20\x7f#]/.test(input.healthPath) ||
        input.healthPath.length > 1024
      )
        throw denied();
      const row = await json(
        "/v13/deployments/" + deploymentID(input.deploymentID),
        credential.token,
        credential.teamID,
      );
      const identity = verified(row, input.releaseID);
      if (identity.deploymentID !== input.deploymentID) throw denied();
      if (row.readyState === "ERROR" || row.readyState === "CANCELED")
        return {
          ...identity,
          status: "failed" as const,
          failureCode: "provider-failed",
        };
      if (row.readyState !== "READY")
        return { ...identity, status: "deploying" as const };
      if (
        !euRegions(row.regions) ||
        (row.passiveRegions !== undefined &&
          !Array.isArray(row.passiveRegions)) ||
        (Array.isArray(row.passiveRegions) &&
          row.passiveRegions.length &&
          !euRegions(row.passiveRegions))
      )
        return {
          ...identity,
          status: "failed" as const,
          failureCode: "region-verification-failed",
        };
      // Provider deployment regions are actual placement, while function metadata may override defaults.
      if (row.functions !== undefined && row.functions !== null) {
        if (typeof row.functions !== "object" || Array.isArray(row.functions))
          throw denied();
        for (const fn of Object.values(row.functions) as Record<
          string,
          unknown
        >[]) {
          if (
            !fn ||
            typeof fn !== "object" ||
            (fn.regions !== undefined && !euRegions(fn.regions)) ||
            (fn.functionFailoverRegions !== undefined &&
              (!Array.isArray(fn.functionFailoverRegions) ||
                (fn.functionFailoverRegions.length &&
                  !euRegions(fn.functionFailoverRegions)))) ||
            fn.runtime === "edge"
          )
            return {
              ...identity,
              status: "failed" as const,
              failureCode: "region-verification-failed",
            };
        }
      }
      if (row.aliasAssigned !== true || row.aliasError)
        return { ...identity, status: "deploying" as const };
      const probe = async (origin: string) => {
        const health = new URL(input.healthPath, origin);
        if (health.origin !== origin) throw denied();
        try {
          const response = await fetchImpl(health, {
            method: "GET",
            cache: "no-store",
            redirect: "error",
            signal: AbortSignal.timeout(10000),
            headers: { Accept: "text/plain" },
          });
          await response.body?.cancel();
          return response.status === 200;
        } catch {
          return false;
        }
      };
      let healthy = await probe(identity.url);
      if (!healthy) {
        // Unique URLs can require Vercel SSO while the production alias is public.
        // Only provider-owned vercel.app DNS is eligible; custom hosts/redirects are never followed.
        try {
          const aliases = await json(
            "/v2/deployments/" + identity.deploymentID + "/aliases",
            credential.token,
            credential.teamID,
          );
          const hosts: string[] = Array.isArray(aliases.aliases)
            ? aliases.aliases
                .map((alias: unknown) =>
                  alias && typeof alias === "object" && "alias" in alias
                    ? alias.alias
                    : undefined,
                )
                .filter(
                  (host: unknown): host is string =>
                    typeof host === "string" &&
                    /^[a-z0-9][a-z0-9-]{0,62}\.vercel\.app$/.test(host),
                )
            : [];
          for (const host of [...new Set(hosts)]
            .sort((a, b) => a.length - b.length)
            .slice(0, 3)) {
            const path = "/v4/aliases/" + encodeURIComponent(host);
            const before = await json(
              path,
              credential.token,
              credential.teamID,
            );
            const matches = (alias: Record<string, unknown>) =>
              alias.alias === host &&
              alias.projectId === credential.projectID &&
              alias.deploymentId === identity.deploymentID &&
              !alias.deletedAt &&
              !alias.redirect &&
              (!alias.deployment ||
                (typeof alias.deployment === "object" &&
                  "id" in alias.deployment &&
                  alias.deployment.id === identity.deploymentID)) &&
              typeof alias.uid === "string" &&
              Number.isSafeInteger(alias.updatedAt);
            if (!matches(before) || !(await probe("https://" + host))) continue;
            const after = await json(path, credential.token, credential.teamID);
            // Recheck assignment and version so a newer healthy deployment cannot validate this release.
            if (
              matches(after) &&
              after.uid === before.uid &&
              after.updatedAt === before.updatedAt
            ) {
              healthy = true;
              break;
            }
          }
        } catch {
          /* Missing alias access cannot prove deployment health. */
        }
      }
      if (!healthy)
        return {
          ...identity,
          status: "failed" as const,
          failureCode: "healthcheck-failed",
        };
      return { ...identity, status: "ready" as const, region: "fra1" as const };
    }
    return { resolveDeployment, reconcileDeployment, observeDeployment };
  }
  /** Historical releases without a health snapshot retain their original root check. */
  function gitReleaseHealthPath(
    target:
      | {
          healthPath?: unknown;
        }
      | null
      | undefined,
  ): string {
    return gitHealthPath.parse(target?.healthPath ?? "/");
  }
  return { createGitVercelObserver, gitReleaseHealthPath };
}
