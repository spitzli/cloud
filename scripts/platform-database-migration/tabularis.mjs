#!/usr/bin/env node
/** Targeted patch preparation only. This script never calls kubectl or mutates a cluster. */
import fs from "node:fs";
import path from "node:path";
import { X509Certificate } from "node:crypto";
import { pathToFileURL } from "node:url";
export const TARGET = Object.freeze({
  name: "db-webdock-operator",
  namespace: "spitzli-databases",
  connectionID: "8032eaca-fddf-4c95-8d12-1b29e2b2dc8e",
  host: "db-103217546630533120.pg.webdock.dev",
  port: 32000,
  address: "79.143.177.157",
  database: "db_103217546714419200",
  user: "webdock_browser_website",
});
const requireValue = (value) => {
  if (!value) throw new Error("Unexpected Tabularis cutover input");
};
function resource(snapshot, kind) {
  const rows = snapshot?.items?.filter(
    (row) =>
      row.kind === kind &&
      row.metadata?.name === TARGET.name &&
      row.metadata?.namespace === TARGET.namespace,
  );
  requireValue(
    rows?.length === 1 && typeof rows[0].metadata.resourceVersion === "string",
  );
  return rows[0];
}
const version = (row) => ({
  op: "test",
  path: "/metadata/resourceVersion",
  value: row.metadata.resourceVersion,
});
export function preparePatches(snapshot, caFile) {
  const deployment = resource(snapshot, "Deployment"),
    network = resource(snapshot, "NetworkPolicy");
  requireValue(
    JSON.stringify(network.spec.podSelector) ===
      JSON.stringify({ matchLabels: { app: TARGET.name } }),
  );
  const pod = deployment.spec.template.spec,
    index = pod.containers.findIndex((c) => c.name === "tabularis");
  requireValue(
    index >= 0 &&
      pod.containers[index].image === "webdock-tabularis:production-fdd6c0a6",
  );
  requireValue(
    Array.isArray(pod.volumes) &&
      Array.isArray(pod.containers[index].volumeMounts) &&
      Array.isArray(network.spec.egress),
  );
  requireValue(
    !pod.volumes.some((v) => v.name === "database-trust") &&
      !pod.containers[index].volumeMounts.some(
        (m) => m.name === "database-trust" || m.mountPath === "/database-trust",
      ),
  );
  const ca = fs.readFileSync(caFile, "utf8");
  requireValue(
    /^\s*(?:-----BEGIN CERTIFICATE-----\s+[A-Za-z0-9+/=\s]+-----END CERTIFICATE-----\s*)+$/.test(
      ca,
    ),
  );
  for (const pem of ca.match(
    /-----BEGIN CERTIFICATE-----[\s\S]+?-----END CERTIFICATE-----/g,
  ))
    requireValue(new X509Certificate(pem).ca);
  const volume = {
    name: "database-trust",
    configMap: { name: TARGET.name + "-database-ca" },
  };
  const mount = {
    name: "database-trust",
    mountPath: "/database-trust",
    readOnly: true,
  };
  const egress = [
    {
      to: [{ ipBlock: { cidr: TARGET.address + "/32" } }],
      ports: [{ protocol: "TCP", port: TARGET.port }],
    },
  ];
  const mountPath = `/spec/template/spec/containers/${index}/volumeMounts`;
  return {
    configMap: {
      apiVersion: "v1",
      kind: "ConfigMap",
      metadata: {
        name: TARGET.name + "-database-ca",
        namespace: TARGET.namespace,
      },
      data: { "ca.crt": ca },
    },
    deployment: [
      version(deployment),
      {
        op: "test",
        path: `/spec/template/spec/containers/${index}/name`,
        value: "tabularis",
      },
      { op: "add", path: "/spec/template/spec/volumes/-", value: volume },
      { op: "add", path: mountPath + "/-", value: mount },
    ],
    network: [
      version(network),
      { op: "replace", path: "/spec/egress", value: egress },
    ],
    rollbackDeployment: [
      {
        op: "test",
        path: `/spec/template/spec/volumes/${pod.volumes.length}`,
        value: volume,
      },
      {
        op: "test",
        path: mountPath + "/" + pod.containers[index].volumeMounts.length,
        value: mount,
      },
      {
        op: "remove",
        path: mountPath + "/" + pod.containers[index].volumeMounts.length,
      },
      {
        op: "remove",
        path: `/spec/template/spec/volumes/${pod.volumes.length}`,
      },
    ],
    rollbackNetwork: [
      { op: "test", path: "/spec/egress", value: egress },
      { op: "replace", path: "/spec/egress", value: network.spec.egress },
    ],
  };
}
export function connectionPatch(snapshot, credentials) {
  requireValue(
    credentials &&
      Object.keys(credentials).sort().join(",") ===
        "database,password,username" &&
      credentials.database === TARGET.database &&
      credentials.username === TARGET.user &&
      typeof credentials.password === "string" &&
      credentials.password.length >= 32 &&
      !credentials.password.includes("\0"),
  );
  const secret = resource(snapshot, "Secret"),
    before = secret.data?.["connections.json"];
  requireValue(typeof before === "string");
  const saved = JSON.parse(Buffer.from(before, "base64").toString("utf8"));
  requireValue(
    saved &&
      Array.isArray(saved.connections) &&
      saved.connections.length === 1 &&
      Array.isArray(saved.groups),
  );
  const connection = saved.connections[0];
  requireValue(
    connection.id === TARGET.connectionID &&
      connection.params?.driver === "postgres",
  );
  // Preserve envelope, groups, connection identity and every unrelated connection parameter.
  Object.assign(connection.params, {
    host: TARGET.host,
    port: TARGET.port,
    database: TARGET.database,
    username: TARGET.user,
    password: credentials.password,
    ssl_mode: "verify-full",
    ssl_ca: "/database-trust/ca.crt",
  });
  const after = Buffer.from(JSON.stringify(saved)).toString("base64");
  return {
    forward: [
      version(secret),
      { op: "replace", path: "/data/connections.json", value: after },
    ],
    rollback: [
      { op: "test", path: "/data/connections.json", value: after },
      { op: "replace", path: "/data/connections.json", value: before },
    ],
  };
}
export function main([command, snapshotFile, inputFile, outputDirectory]) {
  requireValue(
    ["prepare", "credentials"].includes(command) &&
      snapshotFile &&
      inputFile &&
      outputDirectory,
  );
  if (command === "credentials")
    requireValue((fs.statSync(inputFile).mode & 0o077) === 0);
  const snapshot = JSON.parse(fs.readFileSync(snapshotFile, "utf8"));
  const result =
    command === "prepare"
      ? preparePatches(snapshot, inputFile)
      : connectionPatch(
          snapshot,
          JSON.parse(fs.readFileSync(inputFile, "utf8")),
        );
  fs.mkdirSync(outputDirectory, { mode: 0o700, recursive: true });
  const entries =
    command === "prepare"
      ? Object.entries(result)
      : [
          ["secret", result.forward],
          ["rollbackSecret", result.rollback],
        ];
  for (const [name] of entries)
    requireValue(!fs.existsSync(path.join(outputDirectory, name + ".json")));
  for (const [name, value] of entries)
    fs.writeFileSync(
      path.join(outputDirectory, name + ".json"),
      JSON.stringify(value, null, 2),
      { flag: "wx", mode: 0o600 },
    );
}
if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    main(process.argv.slice(2));
    console.log("Private targeted patches prepared; no cluster changes made.");
  } catch {
    console.error(
      "Tabularis patch preparation failed closed; inspect private inputs without printing credentials.",
    );
    process.exitCode = 1;
  }
}
