#!/usr/bin/env bun
import {
  mkdir,
  writeFile,
  readFile,
  readdir,
  cp,
  lstat,
  rename,
} from "node:fs/promises";
import { resolve, dirname, join, basename } from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { parsePluginManifest } from "@webdock/plugin-sdk/contracts";
const args = process.argv.slice(2);
const json = args.includes("--json");
const option = (name) => {
  const i = args.indexOf(name);
  return i < 0 ? undefined : args[i + 1];
};
const print = (x) =>
  console.log(
    json
      ? JSON.stringify(x)
      : typeof x === "string"
        ? x
        : JSON.stringify(x, null, 2),
  );
const help =
  "webdock plugin create <name> --template actions --dir <empty-dir>\nwebdock plugin create <name> --template backend --services jobs --dir <empty-dir>\nwebdock plugin validate [--dir <dir>] [--json]\nwebdock plugin build [--dir <dir>]\nwebdock plugin pack --image <repository@sha256:digest> [--dir <dir>] [--json]\nAll commands are local. No publish/install/dev command is implemented yet.";
try {
  if (args.includes("--help") || args.length === 0) {
    print(help);
    process.exit(0);
  }
  if (args[0] !== "plugin") throw Error(help);
  const command = args[1];
  const dir = resolve(option("--dir") ?? ".");
  const source = join(dir, "tabularium.json");
  if (command === "create") {
    const name = args[2];
    if (!/^[a-z][a-z0-9-]{0,63}$/.test(name ?? ""))
      throw Error("Use a lowercase extension name");
    const templateName = option("--template") ?? "backend";
    if (!["backend", "actions"].includes(templateName) ||
        (templateName === "backend" && (option("--services") ?? "jobs") !== "jobs") ||
        (templateName === "actions" && option("--services")))
      throw Error("Choose actions without services, or backend with jobs");
    if (!option("--dir")) throw Error("create requires --dir");
    let exists = false;
    try {
      const stat = await lstat(dir);
      exists = true;
      if (
        !stat.isDirectory() ||
        stat.isSymbolicLink() ||
        (await readdir(dir)).length
      )
        throw Error("Destination must be an empty real directory");
    } catch (e) {
      if (e.code !== "ENOENT") throw e;
    }
    const manifest = {
      name,
      version: "0.1.0",
      kind: "extension",
      description: "Background-job integration fixture",
      protocolVersion: 1,
      sdkVersion: 1,
      dataProtocolVersion: 1,
      coreVersion: ">=0.1.0 <1.0.0",
      image: "ghcr.io/spitzli/" + name + "@sha256:" + "0".repeat(64),
      dependencies: [],
      capabilities: [
        "plugin.jobs.read",
        "plugin.jobs.enqueue",
        "plugin.jobs.cancel",
      ],
      schemaVersion: 1,
      readableSchema: { min: 1, max: 1 },
      migrations: [],
      tables: [],
      services: { required: [{ name: "jobs", version: 1 }], optional: [] },
      configuration: {
        type: "object",
        properties: {},
        additionalProperties: false,
      },
      actions: [
        {
          name: "demo.enqueue",
          intent: "write",
          capabilities: ["plugin.jobs.enqueue"],
        },
      ],
      jobs: [{ name: "demo.process", capabilities: [] }],
      navigation: [],
      slots: [],
      events: [],
      secrets: [],
      httpBindings: [],
    };
    if (templateName === "actions") {
      manifest.description = "Stateless action plugin";
      manifest.capabilities = [];
      manifest.services = { required: [], optional: [] };
      manifest.actions = [{ name: "demo.echo", intent: "read", capabilities: [] }];
      manifest.jobs = [];
    }
    parsePluginManifest(manifest);
    if (!exists) await mkdir(dir, { recursive: true });
    const template = fileURLToPath(
      new URL(`../templates/${templateName}/`, import.meta.url),
    );
    for (const entry of await readdir(template))
      await cp(join(template, entry), join(dir, entry), {
        recursive: true,
        errorOnExist: true,
        force: false,
      });
    const pkg = {
      name,
      version: "0.1.0",
      private: true,
      type: "module",
      scripts: { build: "tsc -p tsconfig.json", start: "bun dist/index.js" },
      dependencies: { "@webdock/plugin-sdk": "0.1.0" },
      devDependencies: { typescript: "^5", "@types/bun": "^1" },
    };
    if (templateName === "actions") {
      const sdkRoot = resolve(dirname(fileURLToPath(import.meta.resolve("@webdock/plugin-sdk/contracts"))), "../..");
      const vendor = join(dir, "vendor/plugin-sdk");
      await mkdir(vendor, { recursive: true });
      await cp(join(sdkRoot, "dist"), join(vendor, "dist"), { recursive: true });
      const sdkPackage = JSON.parse(await readFile(join(sdkRoot, "package.json"), "utf8"));
      // The vendored distribution is already compiled; local-file installs can run prepare even with ignore-scripts.
      delete sdkPackage.scripts;
      delete sdkPackage.devDependencies;
      await writeFile(join(vendor, "package.json"), JSON.stringify(sdkPackage, null, 2) + "\n");
      for (const entry of await readdir(sdkRoot)) {
        if (/^(?:license|notice|copying)(?:[.-].*)?$/i.test(entry))
          await cp(join(sdkRoot, entry), join(vendor, entry));
      }
      pkg.dependencies["@webdock/plugin-sdk"] = "file:vendor/plugin-sdk";
    }
    await writeFile(
      join(dir, "package.json"),
      JSON.stringify(pkg, null, 2) + "\n",
      { flag: "wx" },
    );
    await writeFile(
      join(dir, "tsconfig.json"),
      JSON.stringify(
        {
          compilerOptions: {
            target: "ES2022",
            module: "NodeNext",
            moduleResolution: "NodeNext",
            strict: true,
            outDir: "dist",
            rootDir: "src",
            skipLibCheck: true,
          },
          include: ["src/**/*.ts"],
        },
        null,
        2,
      ) + "\n",
      { flag: "wx" },
    );
    await writeFile(source, JSON.stringify(manifest, null, 2) + "\n", {
      flag: "wx",
    });
    const readme = join(dir, "README.md");
    await writeFile(
      readme,
      (await readFile(readme, "utf8")).replaceAll("__NAME__", name),
    );
    await writeFile(join(dir, ".gitignore"), (templateName === "actions" ? "node_modules/\n/dist/\n.env*\n" : "node_modules/\ndist/\n.env*\n"), {
      flag: "wx",
    });
    await writeFile(
      join(dir, ".dockerignore"),
      "node_modules\ndist\n.git\n.env*\n",
    );
    print({ created: dir, name, installed: false });
  } else if (command === "validate" || command === "pack") {
    const raw = JSON.parse(await readFile(source, "utf8"));
    if (command === "pack") {
      const image = option("--image");
      if (!image || /@sha256:0{64}$/.test(image))
        throw Error("pack requires a real --image digest");
      raw.image = image;
    }
    const manifest = parsePluginManifest(raw);
    if (command === "validate") {
      print({ valid: true, name: manifest.name });
    } else {
      const canonical = (value) =>
        value && typeof value === "object"
          ? Array.isArray(value)
            ? value.map(canonical)
            : Object.fromEntries(
                Object.entries(value)
                  .sort(([a], [b]) => a.localeCompare(b))
                  .map(([k, v]) => [k, canonical(v)]),
              )
          : value;
      const bytes = JSON.stringify(canonical(manifest)) + "\n";
      const out = join(dir, "dist");
      await mkdir(out, { recursive: true });
      for (const name of ["tabularium.json", manifest.name + "-universal.json"])
        await writeFile(join(out, name), bytes);
      const sha256 = createHash("sha256").update(bytes).digest("hex");
      await writeFile(
        join(out, "release-lock.json"),
        JSON.stringify(
          {
            name: manifest.name,
            version: manifest.version,
            image: manifest.image,
            manifestSha256: sha256,
            descriptorSha256: sha256,
          },
          null,
          2,
        ) + "\n",
      );
      print({ packed: out, sha256, published: false });
    }
  } else if (command === "build") {
    parsePluginManifest(JSON.parse(await readFile(source, "utf8")));
    const result = spawnSync(
      "bun",
      ["run", "build"],
      { cwd: dir, stdio: json ? "pipe" : "inherit" },
    );
    if (result.status !== 0) throw Error("Plugin build failed");
    print({ built: dir });
  } else throw Error("Unknown plugin command. " + help);
} catch (error) {
  console.error(
    json ? JSON.stringify({ error: error.message }) : error.message,
  );
  process.exitCode = 1;
}
