import { readdir, realpath, stat } from "node:fs/promises";
import { extname, isAbsolute, join, relative } from "node:path";
import { pathToFileURL } from "node:url";
import Elysia from "elysia";
import { fsr } from "elysia-fsr";
export type RouteEntry = { method: string; path: string; source: string };
const normalize = (path: string) =>
  "/" + path.split("/").filter(Boolean).join("/");
const key = (entry: Pick<RouteEntry, "method" | "path">) =>
  `${entry.method} ${normalize(entry.path).replace(/:[^/]+/g, ":param")}`;
async function files(root: string, dir = root): Promise<string[]> {
  const result: string[] = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isSymbolicLink())
      throw Error(`Route path cannot be a symlink: ${relative(root, path)}`);
    if (entry.name.startsWith("."))
      throw Error(
        `Hidden route path must be mounted explicitly: ${relative(root, path)}`,
      );
    if (entry.isDirectory()) result.push(...(await files(root, path)));
    else if (/\.(ts|js|mjs)$/.test(entry.name)) result.push(path);
    else throw Error(`Unsupported route path: ${relative(root, path)}`);
  }
  return result.sort();
}
function prefix(file: string): string {
  const parts = file.slice(0, -extname(file).length).split("/");
  if (/^\(.+\)$/.test(parts.at(-1)!))
    throw Error(
      `Ghost controllers are not security boundaries; unsupported route path: ${file}`,
    );
  if (parts.at(-1) === "index") parts.pop();
  return normalize(
    parts
      .filter((part) => !/^\([a-zA-Z0-9_-]+\)$/.test(part))
      .map((part) => {
        if (part === "[...]") return "*";
        if (/^\[[a-zA-Z_][a-zA-Z0-9_]*\]$/.test(part))
          return ":" + part.slice(1, -1);
        if (!/^[a-zA-Z0-9_-]+$/.test(part))
          throw Error(`Unsupported route path: ${file}`);
        return part;
      })
      .join("/"),
  );
}
/** Fail before FSR's lossy merge; authorization belongs on statically mounted parents. */
export async function loadRoutes(
  root: string,
): Promise<{ app: Elysia; manifest: RouteEntry[] }> {
  if (
    !isAbsolute(root) ||
    !(await stat(root)).isDirectory() ||
    (await realpath(root)) !== root
  )
    throw Error("An absolute, real route directory is required");
  const paths = await files(root);
  if (!paths.length) throw Error("Required route tree is empty");
  const manifest: RouteEntry[] = [];
  const seen = new Map<string, string>();
  for (const path of paths) {
    const source = relative(root, path),
      base = prefix(source);
    const module = await import(pathToFileURL(path).href);
    if (!(module.default instanceof Elysia))
      throw Error(`Route must export an Elysia instance: ${source}`);
    if (!module.default.routes.length)
      throw Error(`Route module is empty: ${source}`);
    for (const route of module.default.routes) {
      const entry = {
        method: route.method,
        path: normalize(base + "/" + route.path),
        source,
      };
      for (const method of entry.method === "ALL"
        ? [
            "GET",
            "POST",
            "PUT",
            "PATCH",
            "DELETE",
            "HEAD",
            "OPTIONS",
            "CONNECT",
            "TRACE",
          ]
        : [entry.method]) {
        const identity = key({ ...entry, method });
        if (seen.has(identity))
          throw Error(
            `Route collision: ${seen.get(identity)} and ${source} (${identity})`,
          );
        seen.set(identity, source);
      }
      manifest.push(entry);
    }
  }
  const app = await fsr({
    dir: root,
    filter: "**/*.{ts,js,mjs}",
    types: false,
    logLevel: -1,
  });
  const actual = app.routes.map(key).sort();
  if (JSON.stringify(actual) !== JSON.stringify(manifest.map(key).sort()))
    throw Error("FSR route manifest mismatch");
  app.compile();
  return { app, manifest };
}
