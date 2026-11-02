import { test, expect } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { join } from "node:path";
import { buildArtifact } from "../scripts/build";
import { loadRoutes } from "../src/route-preflight";
test("release artifact includes filesystem routes and their backend imports", async () => {
  const directory = await mkdtemp(join(import.meta.dir, ".artifact-"));
  try {
    await buildArtifact(directory);
    const { manifest } = await loadRoutes(join(directory, "routes/operator"));
    expect(
      manifest.some((route) => route.path === "/registry/:collection/:id"),
    ).toBe(true);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
