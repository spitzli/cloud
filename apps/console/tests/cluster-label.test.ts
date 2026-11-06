import assert from "node:assert/strict";
import { test } from "node:test";
import { clusterLabel } from "../src/cluster-label";

await test("legacy platform cluster labels use Spitzli Cloud without changing customer names", () => {
  assert.equal(clusterLabel("Webdock Cluster EU"), "Spitzli Cloud Cluster EU");
  assert.equal(clusterLabel("Spitzli Cloud Cluster EU"), "Spitzli Cloud Cluster EU");
  assert.equal(clusterLabel("Schattenclan private cluster"), "Schattenclan private cluster");
  assert.equal(clusterLabel("Webdock Labs"), "Webdock Labs");
});
