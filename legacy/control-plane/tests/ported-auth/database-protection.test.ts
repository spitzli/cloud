import test from "node:test";
import assert from "node:assert/strict";
import { assertDatabaseMaintenanceAllowed } from "../../src/modules/hosting/database-protection";
test("control-plane database maintenance cannot stop its own job ledger", () => {
  for (const action of [
    "pause",
    "restart",
    "resize",
    "restore",
    "delete",
    "purge",
    "rotateCredentials",
    "requestCancellation",
    "upgrade",
  ])
    assert.throws(
      () =>
        assertDatabaseMaintenanceAllowed("123", "databases." + action, "123"),
      /offline maintenance/,
    );
  for (const action of ["get", "list", "reconcile", "resume"])
    assert.doesNotThrow(() =>
      assertDatabaseMaintenanceAllowed("123", "databases." + action, "123"),
    );
  assert.doesNotThrow(() =>
    assertDatabaseMaintenanceAllowed("456", "databases.pause", "123"),
  );
});
