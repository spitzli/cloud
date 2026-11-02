import { execFileSync } from "node:child_process";
import { arch, platform as hostOS } from "node:os";
import { randomUUID, createHash } from "node:crypto";
import { writeFile, readFile } from "node:fs/promises";
import { join } from "node:path";
import { coreRuntimeProductPath } from "./build-core-console.mjs";
import {
  images,
  databaseImages,
  cmsImages,
  cmsVersions,
  inspectOCIArchive,
  executionChecks,
  evidenceChecks,
} from "./manifest.mjs";
const [directory, platform, revision] = process.argv.slice(2);
const includeCMS = process.env.WEBDOCK_RELEASE_CMS === "true";
const cmsRuntime = includeCMS ? cmsVersions : undefined;
const includeDatabases = process.env.WEBDOCK_RELEASE_DATABASES === "true";
const pin = JSON.parse(
  await readFile(
    new URL("../../vendor/tabularis-web-ui.json", import.meta.url),
    "utf8",
  ),
);
const databaseRuntime = includeDatabases
  ? {
      sourceCommit: pin.sourceCommit,
      sourceTree: pin.sourceTree,
      sourceImage: process.env.TABULARIS_IMAGE,
    }
  : undefined;
const selectedImages = [...images, ...(includeDatabases ? databaseImages : []), ...(includeCMS ? cmsImages : [])];
const architecture = { x64: "amd64", arm64: "arm64" }[arch()];
if (hostOS() !== "linux" || platform !== `linux/${architecture}`)
  throw Error(
    "Architecture execution requires a matching native Linux runner; emulation is not support evidence",
  );
const dockerPlatform = execFileSync(
  "docker",
  ["info", "--format", "{{.OSType}}/{{.Architecture}}"],
  { encoding: "utf8" },
)
  .trim()
  .replace("/x86_64", "/amd64")
  .replace("/aarch64", "/arm64");
if (dockerPlatform !== platform)
  throw Error("Docker engine architecture differs from native runner");
if (!/^[a-f0-9]{40}$/.test(revision))
  throw Error("Full source revision required");
const prefix = "webdock-release-" + randomUUID().replaceAll("-", ""),
  tags = [],
  checks = [],
  imageIndexDigests = {},
  imageRegistryDigests = {};
const restrictions = [
  "--network",
  "none",
  "--read-only",
  "--cap-drop",
  "ALL",
  "--security-opt",
  "no-new-privileges",
  "--tmpfs",
  "/tmp:rw,nosuid,size=64m,mode=1777",
];
const run = (name, entrypoint, args) =>
  execFileSync(
    "docker",
    [
      "run",
      "--rm",
      ...restrictions,
      "--entrypoint",
      entrypoint,
      `${prefix}:${name}`,
      ...args,
    ],
    { stdio: "inherit", timeout: 120000 },
  );
try {
  for (const name of selectedImages) {
    const file = join(directory, name + ".oci.tar"),
      oci = inspectOCIArchive(file);
    if (oci.manifests.some((item) => item.platform !== platform))
      throw Error("Archive platform does not match native runner");
    imageIndexDigests[name] = oci.indexDigest;
    imageRegistryDigests[name] = oci.registryDigest;
    const tag = `${prefix}:${name}`;
    execFileSync(
      "skopeo",
      ["copy", `oci-archive:${file}`, `docker-daemon:${tag}`],
      { stdio: "inherit", timeout: 300000 },
    );
    tags.push(tag);
    if (
      execFileSync(
        "docker",
        ["image", "inspect", tag, "--format", "{{.Os}}/{{.Architecture}}"],
        { encoding: "utf8" },
      ).trim() !== platform
    )
      throw Error("Loaded image architecture mismatch");
  }
  const licenseDigests = Object.fromEntries(await Promise.all(["LICENSE", "COMMERCIAL-LICENSING.md", "NOTICE"].map(async name => [name, createHash("sha256").update(await readFile(name)).digest("hex")])));
  for (const name of ["api-core", "console-core", "api", "console", "auth", "worker", "migrate", ...(includeCMS ? cmsImages : [])])
    run(name, ["console-core", "console", "auth", "cms-instance"].includes(name) ? "node" : "bun", ["-e", `const fs=require('node:fs'),crypto=require('node:crypto');for(const [name,expected] of Object.entries(${JSON.stringify(licenseDigests)}))if(crypto.createHash('sha256').update(fs.readFileSync('/usr/share/licenses/webdock/'+name)).digest('hex')!==expected)throw Error('License notice differs: '+name);`]);
  run("agent", "python3", ["-c", "import hashlib,json,pathlib,sys; expected=json.loads(sys.argv[1]); assert all(hashlib.sha256((pathlib.Path('/usr/share/licenses/webdock')/name).read_bytes()).hexdigest()==digest for name,digest in expected.items())", JSON.stringify(licenseDigests)]);
  checks.push({ name: "webdock-license-notices", passed: true });
  run("api", "bun", ["test", "/app/packages/module-sdk/src/index.test.ts"]);
  run("api-core", "bun", ["-e", "const fs=require('node:fs');if(JSON.stringify(fs.readdirSync('/app'))!==JSON.stringify(['core.js']))throw Error('Unexpected files in Core API runtime');if(fs.statSync('/app/core.js').size===0)throw Error('Empty Core API bundle');"]);
  run("console-core", "node", ["-e", `const fs=require('node:fs');const isProduct=${coreRuntimeProductPath.toString()};function walk(path){for(const entry of fs.readdirSync(path,{withFileTypes:true})){const child=path+'/'+entry.name;if(isProduct(child))throw Error('Product file in Core Console: '+child);if(entry.isDirectory())walk(child);}}walk('/app');if(!fs.existsSync('/app/.core-build/console/server.js'))throw Error('Missing Core Console entry');`]);
  checks.push({ name: "core-product-boundary", passed: true });
  execFileSync(process.execPath, ["tools/release/smoke-core-images.mjs", ...["api-core", "console-core", "auth", "migrate"].map(name => `${prefix}:${name}`)], { stdio: "inherit", timeout: 300000 });
  checks.push({ name: "core-images-fresh-installation", passed: true });
  checks.push({ name: "api-module-tests", passed: true });
  run("api", "bun", [
    "-e",
    `import {readConfig} from '/app/apps/control-plane/dist/config.ts';
const {configureFullInstallation}=await import('/app/apps/control-plane/dist/full-installation.ts');
configureFullInstallation();
const inputs=[{name:'one',domains:'auth,mail',modules:{mail:{enabled:true}}},{name:'two',domains:'auth,hosting,databases',modules:{hosting:{enabled:true},databases:{enabled:true}}}];
const values=inputs.map(input=>readConfig({NODE_ENV:'production',WEBDOCK_API_ORIGIN:'https://api.'+input.name+'.example',BETTER_AUTH_URL:'https://auth.'+input.name+'.example',WEBDOCK_STUDIO_ORIGIN:'https://studio.'+input.name+'.example',WEBDOCK_CONSOLE_ORIGIN:'https://console.'+input.name+'.example',WEBDOCK_API_DOMAINS:input.domains,WEBDOCK_MODULES:JSON.stringify({configVersion:1,modules:input.modules})}));
if(values[0].apiOrigin===values[1].apiOrigin||values[0].modules.modules.mail!=='enabled'||values[1].modules.modules.mail!=='disabled'||values[1].modules.modules.databases!=='enabled')throw Error('Runtime configuration was retained across installations');`,
  ]);
  checks.push({ name: "api-runtime-configurations", passed: true });
  run("worker", "bun", [
    "test",
    "/app/apps/control-plane/tests/worker-pool.test.ts",
  ]);
  checks.push({ name: "worker-pool-tests", passed: true });
  run("migrate", "bun", ["test", "/app/tools/migrations/config.test.ts"]);
  checks.push({ name: "migration-config-tests", passed: true });
  for (const [name, app, port] of [
    ["console", "console", 3120],
    ["auth", "auth", 3125],
  ]) {
    const container = prefix + "-" + name;
    execFileSync(
      "docker",
      ["run", "-d", "--name", container, ...restrictions, `${prefix}:${name}`],
      { stdio: "ignore" },
    );
    try {
      const code = `(async()=>{const fs=require('node:fs');const f=fs.readdirSync('/app/apps/${app}/.next/static/chunks').find(f=>f.endsWith('.js'));for(let i=0;i<30;i++){try{if((await fetch('http://127.0.0.1:${port}/_next/static/chunks/'+f)).status===200)return;}catch{}await new Promise(r=>setTimeout(r,1000));}process.exit(1)})()`;
      execFileSync("docker", ["exec", container, "bun", "-e", code], {
        stdio: "inherit",
        timeout: 45000,
      });
      checks.push({ name: name + "-http-assets", passed: true });
    } finally {
      execFileSync("docker", ["rm", "-f", container], { stdio: "ignore" });
    }
  }
  run("agent", "python3", ["daemon.py", "--help"]);
  checks.push({ name: "agent-entrypoint", passed: true });
  run("mail-runtime", "/usr/local/bin/stalwart", ["--version"]);
  checks.push({ name: "mail-runtime-version", passed: true });
  if (includeDatabases) {
    run("tabularis-runtime", "/usr/local/bin/tabularis", ["--version"]);
    checks.push({ name: "tabularis-runtime-version", passed: true });
    execFileSync(
      process.execPath,
      [
        "test",
        "./apps/database-gateway/tests/managed-real-runtime.test.ts",
      ],
      {
        stdio: "inherit",
        timeout: 120000,
        env: {
          ...process.env,
          WEBDOCK_MANAGED_RUNTIME_TEST_IMAGE: `${prefix}:database-gateway`,
        },
      },
    );
    checks.push({ name: "database-gateway-managed-runtime", passed: true });
  }
  if (includeCMS) {
    run("cms-instance", "node", ["-e", `const fs=require('node:fs');if(process.versions.node.split('.')[0]!=='24'||JSON.parse(fs.readFileSync('/app/node_modules/payload/package.json','utf8')).version!==${JSON.stringify(cmsVersions.payloadVersion)})throw Error('CMS runtime version differs from pinned evidence');`]);
    execFileSync("python3", ["apps/cms-instance/scripts/acceptance.py", "--image", `${prefix}:cms-instance`], { stdio: "inherit", timeout: 600000 });
    checks.push({ name: "cms-instance-fresh-postgres", passed: true });
    execFileSync(process.execPath, ["extensions/managed-cms/tests/image-smoke.mjs"], { stdio: "inherit", timeout: 120000, env: { ...process.env, PLUGIN_TEST_IMAGE: `${prefix}:managed-cms` } });
    checks.push({ name: "managed-cms-runtime", passed: true });
  }
  if (
    evidenceChecks({ databaseRuntime, cmsRuntime }).some(
      (name) => !checks.some((check) => check.name === name),
    )
  )
    throw Error("Incomplete architecture check");
  await writeFile(
    join(directory, "architecture-evidence.json"),
    JSON.stringify(
      {
        schemaVersion: 1,
        revision,
        platform,
        runnerPlatform: `linux/${architecture}`,
        emulated: false,
        checks,
        imageIndexDigests,
        imageRegistryDigests,
        ...(databaseRuntime ? { databaseRuntime } : {}),
        ...(cmsRuntime ? { cmsRuntime } : {}),
      },
      null,
      2,
    ) + "\n",
  );
} finally {
  for (const tag of tags)
    execFileSync("docker", ["image", "rm", tag], { stdio: "ignore" });
}
