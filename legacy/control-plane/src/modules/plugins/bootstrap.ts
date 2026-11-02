import type { Pool } from "pg";
import { createRegistry } from "@webdock/module-sdk";
/** Offline-only initialization. Runtime startup must never import or invoke this. */
export async function initializePluginState(
  db: Pick<Pool, "connect">,
  selection: unknown,
  mode: "fresh" | "legacy",
): Promise<void> {
  if (!["fresh", "legacy"].includes(mode))
    throw Error("Invalid initialization mode");
  const c = await db.connect();
  try {
    await c.query("BEGIN");
    await c.query(
      "SELECT pg_advisory_xact_lock(hashtext('webdock-plugin-installation'))",
    );
    if (
      (await c.query("SELECT singleton FROM webdock_auth.plugin_bootstrap"))
        .rows.length
    ) {
      await c.query("COMMIT");
      return;
    }
    if (mode === "fresh" && selection === undefined)
      throw Error("Fresh installation requires an explicit module selection");
    // Legacy import selects the complete retained catalog; fresh empty selection is core-only.
    const selected = selection as { modules?: Record<string, unknown> } | undefined;
    const builtinManifests = mode === "legacy" || (selected?.modules && Object.keys(selected.modules).length)
      ? (await import("../../full-installation")).fullManifests
      : [];
    const registry = createRegistry(builtinManifests, selection === undefined
      ? {configVersion: 1, modules: Object.fromEntries(builtinManifests.map(({id}) => [id, {enabled: true}]))}
      : selection);
    const modules = registry.publicView().modules;
    for (const [id, state] of Object.entries(modules)) {
      const manifest = builtinManifests.find((m) => m.id === id)!;
      const dependencies = manifest.dependencies.map((id) => ({
        id,
        version: builtinManifests.find((m) => m.id === id)!.version,
      }));
      await c.query(
        "INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,dependencies) VALUES($1,'builtin',$2,$2,$3,$4)",
        [id, state, manifest.version, JSON.stringify(dependencies)],
      );
    }
    await c.query(
      "INSERT INTO webdock_auth.plugin_bootstrap(mode) VALUES($1)",
      [mode],
    );
    await c.query("COMMIT");
  } catch (e) {
    await c.query("ROLLBACK");
    throw e;
  } finally {
    c.release();
  }
}
