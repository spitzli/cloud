import test from "node:test";
import assert from "node:assert/strict";
import { preparePatches, connectionPatch, TARGET } from "./tabularis.mjs";
const envelope = {
  connections: [
    {
      id: "8032eaca-fddf-4c95-8d12-1b29e2b2dc8e",
      name: "Website",
      environment: "production",
      params: {
        driver: "postgres",
        host: "old.invalid",
        port: 5432,
        database: "old",
        username: "old-role",
        password: "old-synthetic",
        ssl_mode: "require",
        ssl_ca: null,
        ssh_enabled: false,
      },
    },
  ],
  groups: [{ id: "saved-group", name: "Group" }],
};
const fixture = () => ({
  items: [
    {
      kind: "Deployment",
      metadata: {
        name: "db-webdock-operator",
        namespace: "spitzli-databases",
        resourceVersion: "10",
      },
      spec: {
        template: {
          spec: {
            volumes: [
              {
                name: "state",
                persistentVolumeClaim: { claimName: "retained" },
              },
              {
                name: "credentials",
                secret: { secretName: "db-webdock-operator" },
              },
            ],
            containers: [
              {
                name: "tabularis",
                image: "webdock-tabularis:production-fdd6c0a6",
                volumeMounts: [{ name: "state", mountPath: "/data" }],
              },
              {
                name: "tls",
                image: "unchanged",
                volumeMounts: [{ name: "credentials", mountPath: "/tls" }],
              },
            ],
          },
        },
      },
    },
    {
      kind: "Secret",
      metadata: {
        name: "db-webdock-operator",
        namespace: "spitzli-databases",
        resourceVersion: "11",
      },
      data: {
        "connections.json": Buffer.from(JSON.stringify(envelope)).toString(
          "base64",
        ),
        "proxy-secret": "synthetic-proxy",
        "tls.key": "synthetic-key",
        "tls.crt": "synthetic-cert",
      },
    },
    {
      kind: "NetworkPolicy",
      metadata: {
        name: "db-webdock-operator",
        namespace: "spitzli-databases",
        resourceVersion: "12",
      },
      spec: {
        podSelector: { matchLabels: { app: "db-webdock-operator" } },
        ingress: [{ ports: [{ port: 8443 }] }],
        egress: [
          { to: [{ ipBlock: { cidr: "0.0.0.0/0" } }], ports: [{ port: 5432 }] },
        ],
      },
    },
  ],
});

test("connection replacement preserves identities, groups and unrelated fields without plaintext output", () => {
  const snapshot = fixture(),
    before = structuredClone(snapshot);
  const patches = connectionPatch(snapshot, {
    username: TARGET.user,
    database: TARGET.database,
    password: "new-private-password-12345678901234567890",
  });
  assert.deepEqual(snapshot, before);
  assert.deepEqual(
    patches.forward.map((p) => p.path),
    ["/metadata/resourceVersion", "/data/connections.json"],
  );
  const changed = JSON.parse(
    Buffer.from(patches.forward[1].value, "base64").toString(),
  );
  const expected = structuredClone(envelope);
  Object.assign(expected.connections[0].params, {
    host: TARGET.host,
    port: TARGET.port,
    database: TARGET.database,
    username: TARGET.user,
    password: "new-private-password-12345678901234567890",
    ssl_mode: "verify-full",
    ssl_ca: "/database-trust/ca.crt",
  });
  assert.deepEqual(changed, expected);
  assert.deepEqual(
    patches.rollback[1].value,
    snapshot.items[1].data["connections.json"],
  );
  assert.ok(!JSON.stringify(patches).includes("new-private-password"));
});

test("rejects unexpected connection or identity and malformed credentials", () => {
  for (const field of ["database", "username"])
    assert.throws(() =>
      connectionPatch(fixture(), {
        username: TARGET.user,
        database: TARGET.database,
        password: "a".repeat(40),
        [field]: "unexpected",
      }),
    );
  const snapshot = fixture();
  snapshot.items[1].data["connections.json"] = Buffer.from(
    JSON.stringify({
      ...envelope,
      connections: [...envelope.connections, ...envelope.connections],
    }),
  ).toString("base64");
  assert.throws(() =>
    connectionPatch(snapshot, {
      username: TARGET.user,
      database: TARGET.database,
      password: "a".repeat(40),
    }),
  );
});

test(
  "targeted mount/policy patches preserve existing containers, volumes and ingress",
  { skip: !process.env.TABULARIS_TEST_CA },
  () => {
    // Test CA supplied by integration invocation is a public certificate, not credentials.
    const snapshot = fixture(),
      before = structuredClone(snapshot);
    const patches = preparePatches(snapshot, process.env.TABULARIS_TEST_CA);
    assert.deepEqual(snapshot, before);
    assert.deepEqual(
      patches.deployment.map((p) => p.path),
      [
        "/metadata/resourceVersion",
        "/spec/template/spec/containers/0/name",
        "/spec/template/spec/volumes/-",
        "/spec/template/spec/containers/0/volumeMounts/-",
      ],
    );
    assert.deepEqual(patches.network[1], {
      op: "replace",
      path: "/spec/egress",
      value: [
        {
          to: [{ ipBlock: { cidr: "79.143.177.157/32" } }],
          ports: [{ protocol: "TCP", port: 32000 }],
        },
      ],
    });
    assert.equal(
      patches.configMap.metadata.name,
      "db-webdock-operator-database-ca",
    );
    const patched = structuredClone(snapshot.items[0]);
    patched.spec.template.spec.volumes.push(patches.deployment[2].value);
    patched.spec.template.spec.containers[0].volumeMounts.push(
      patches.deployment[3].value,
    );
    assert.deepEqual(
      patched.spec.template.spec.volumes.slice(0, -1),
      snapshot.items[0].spec.template.spec.volumes,
    );
    assert.deepEqual(
      patched.spec.template.spec.containers[1],
      snapshot.items[0].spec.template.spec.containers[1],
    );
    assert.deepEqual(
      patches.rollbackNetwork[1].value,
      snapshot.items[2].spec.egress,
    );
    assert.ok(!JSON.stringify(patches).includes("synthetic-proxy"));
    assert.ok(!JSON.stringify(patches).includes("synthetic-key"));
    const changed = fixture();
    changed.items[0].spec.template.spec.volumes.push({
      name: "database-trust",
    });
    assert.throws(() => preparePatches(changed, process.env.TABULARIS_TEST_CA));
  },
);
