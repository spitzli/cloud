import semver from "semver";
import type {
  Availability,
  Blocker,
  InstalledPlugin,
  PluginManifest,
} from "@spitzli/plugin-sdk/contracts";
export function dependencyBlockers(
  id: string,
  nextVersion: string | null,
  installed: readonly InstalledPlugin[],
): Blocker[] {
  return installed
    .filter(
      (p) =>
        p.state !== "disabled" &&
        p.dependencies?.some(
          (d) =>
            d.id === id && (nextVersion === null || d.version !== nextVersion),
        ),
    )
    .map((p) => ({
      code: "active_dependent",
      message: `${p.id} requires ${id}@${p.dependencies!.find((d) => d.id === id)!.version}`,
    }));
}
export function checkPluginCompatibility(
  manifest: PluginManifest,
  coreVersion: string,
  installed: readonly InstalledPlugin[],
): Blocker[] {
  const x = manifest;
  const blockers: Blocker[] = [];
  if (
    !semver.valid(coreVersion) ||
    !semver.satisfies(coreVersion, x.coreVersion)
  )
    blockers.push({
      code: "core_incompatible",
      message: `Requires Webdock ${x.coreVersion}`,
    });
  for (const d of x.dependencies)
    if (
      !installed.some(
        (p) =>
          p.id === d.id && p.state === "enabled" && p.version === d.version,
      )
    )
      blockers.push({
        code: "dependency_unavailable",
        message: `Requires enabled ${d.id}@${d.version}`,
      });
  blockers.push(
    ...dependencyBlockers(manifest.name, manifest.version, installed),
  );
  const graph = new Map(installed.map((p) => [p.id, p.dependencies ?? []]));
  graph.set(manifest.name, x.dependencies);
  const visited = new Set<string>(),
    active = new Set<string>();
  const visit = (id: string): boolean => {
    if (active.has(id)) return true;
    if (visited.has(id)) return false;
    active.add(id);
    for (const d of graph.get(id) ?? []) if (visit(d.id)) return true;
    active.delete(id);
    visited.add(id);
    return false;
  };
  if (visit(manifest.name))
    blockers.push({
      code: "dependency_cycle",
      message: "Cyclic plugin dependency",
    });
  return blockers;
}
/** Pure planning only. Callers supply authoritative blockers while holding the admission fence. */
export function planPluginTransition(
  current: InstalledPlugin,
  desired: Availability,
  blockers: readonly Blocker[],
): { state: Availability; desiredState: Availability; blockers: Blocker[] } {
  const reasons = [...blockers];
  if (reasons.some((b) => b.code === "active_dependent"))
    return {
      state: current.state,
      desiredState: current.desiredState,
      blockers: reasons,
    };
  if (desired === "enabled") {
    if (current.kind === "external" && current.runtime !== "ready")
      reasons.push({
        code: "runtime_not_ready",
        message: "Verified runtime readiness is required",
      });
    return {
      state: reasons.length ? current.state : "enabled",
      desiredState: desired,
      blockers: reasons,
    };
  }
  return {
    state: desired === "draining" || reasons.length ? "draining" : "disabled",
    desiredState: desired,
    blockers: reasons,
  };
}

export { parsePluginManifest } from "@spitzli/plugin-sdk/contracts";
export type {
  PluginManifest,
  InstalledPlugin,
  Blocker,
  Availability,
} from "@spitzli/plugin-sdk/contracts";
