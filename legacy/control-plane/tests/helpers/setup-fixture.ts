import { randomBytes } from "node:crypto";
import type { Pool } from "pg";
import type { HostingActor } from "@webdock/hosting-contracts";
export async function createSetupFixture(db: Pool) {
  const base = BigInt(
    "96" + BigInt("0x" + randomBytes(5).toString("hex")).toString(),
  );
  const id = (n: number) => (base + BigInt(n)).toString();
  const [customerID, subject, sessionID, projectID, clusterID] = [
    0, 1, 2, 3, 4,
  ].map(id);
  const credentialHash = randomBytes(32).toString("hex");
  const capacity = {
    apps: 10,
    cpuMillicores: 4000,
    memoryBytes: 4294967296,
    ephemeralBytes: 4294967296,
    volumeBytes: 5368709120,
    replicasPerApp: 10,
    concurrentDeployments: 10,
  };
  await db.query(
    'INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword","twoFactorEnabled") VALUES($1,\'Setup fixture\',$2,\'user\',true,false,false)',
    [subject, subject + "@example.invalid"],
  );
  await db.query(
    'INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval \'1 hour\',now())',
    [sessionID, subject],
  );
  await db.query(
    "INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,'Setup fixture','active')",
    [customerID],
  );
  const org = (
    await db.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customerID],
    )
  ).rows[0].organization_id;
  await db.query(
    'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
    [org, subject],
  );
  await db.query(
    "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES($1,'Setup fixture',$2,'active')",
    [projectID, customerID],
  );
  await db.query(
    "INSERT INTO webdock_auth.hosting_cluster(id,name,provider,country,region,location_evidence,verified,capacity) VALUES($1,'Setup fixture','k3s','DE','local','Disposable test only',true,$2)",
    [clusterID, JSON.stringify(capacity)],
  );
  await db.query(
    "INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,mode,cluster_id,namespace) VALUES($1,$2,'k3s','selfservice',$3,$4)",
    [projectID, customerID, clusterID, "wd-" + projectID],
  );
  await db.query(
    "INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_seen,observation) VALUES($1,$2,1,now(),$3)",
    [
      clusterID,
      credentialHash,
      JSON.stringify({
        capacity,
        nodes: [],
        capabilities: {
          networkIsolation: true,
          restrictedPods: true,
          environment: true,
          serviceBindingsVersion: 1,
          storage: false,
          ingress: false,
        },
      }),
    ],
  );
  await db.query(
    "INSERT INTO webdock_auth.tenant_subscription(customer_id,name,description,base,extras,revision,updated_by) VALUES($1,'Fixture','Local fixture',$2,'{}',1,$3)",
    [customerID, JSON.stringify({ hosting: capacity }), subject],
  );
  const actor: HostingActor = {
    subject,
    sessionID,
    source: "studio",
    scopes: ["hosting:read", "hosting:write"],
  };
  return {
    actor,
    projectID,
    customerID,
    clusterID,
    agent: { clusterID, credentialHash, generation: 1 },
    cleanup: async () => {
      await db.query(
        "DELETE FROM webdock_auth.hosting_setup WHERE project_id=$1",
        [projectID],
      );
      await db.query(
        "DELETE FROM webdock_auth.hosting_service_binding WHERE project_id=$1",
        [projectID],
      );
      await db.query(
        "DELETE FROM webdock_auth.hosting_reservation WHERE project_id=$1",
        [projectID],
      );
      await db.query(
        "DELETE FROM webdock_auth.hosting_operation WHERE project_id=$1",
        [projectID],
      );
      await db.query(
        "DELETE FROM webdock_auth.hosting_app WHERE project_id=$1",
        [projectID],
      );
      await db.query(
        "DELETE FROM webdock_auth.hosting_project WHERE project_id=$1",
        [projectID],
      );
      await db.query(
        "DELETE FROM webdock_auth.hosting_agent WHERE cluster_id=$1",
        [clusterID],
      );
      await db.query("DELETE FROM webdock_auth.hosting_cluster WHERE id=$1", [
        clusterID,
      ]);
      await db.query(
        "DELETE FROM webdock_auth.tenant_subscription WHERE customer_id=$1",
        [customerID],
      );
      await db.query("DELETE FROM webdock_admin.projects WHERE id=$1", [
        projectID,
      ]);
      await db.query(
        'DELETE FROM webdock_auth.member WHERE "organizationId"=$1',
        [org],
      );
      await db.query(
        "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1",
        [customerID],
      );
      await db.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
        customerID,
      ]);
      await db.query("DELETE FROM webdock_auth.organization WHERE id=$1", [
        org,
      ]);
      await db.query("DELETE FROM webdock_auth.session WHERE id=$1", [
        sessionID,
      ]);
      await db.query('DELETE FROM webdock_auth."user" WHERE id=$1', [subject]);
    },
  };
}
