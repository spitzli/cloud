import { test } from "node:test";
import assert from "node:assert/strict";
import { appSchema, settingsCommand } from "../src/hosting-contracts";

const spec = { template:"custom", image:`registry.example/app@sha256:${"a".repeat(64)}`, args:[], port:8080, healthPath:"/", cpuMillicores:100, memoryBytes:67108864, ephemeralBytes:67108864, replicas:1, volumeBytes:0 };
function deployed(value: unknown) {
  const result = appSchema.parse(value);
  if (result.lifecycle !== "deployed") throw new Error("Fixture must be deployed");
  return result;
}
const app = deployed({ id:"301", projectID:"201", name:"App", lifecycle:"deployed", spec, revision:1, observedRevision:1, status:"ready", operationID:null, logs:null, lastError:null, subscriptionRevision:1, operator:false, canManage:true, canRequestLogs:true, automaticScalingAvailable:true });
function form() {
  const data = new FormData();
  for (const [key,value] of Object.entries({image:spec.image,args:"[]",port:"8080",healthPath:"/",cpuMillicores:"0.1 cores",memoryBytes:"64 MB",ephemeralBytes:"64 MB",replicas:"1",scalingMode:"automatic",minReplicas:"2",maxReplicas:"5",targetCPUUtilizationPercent:"70"})) data.set(key,value);
  return data;
}
await test("settings persist min/max policy while stop intent stays stopped", () => {
  const command = settingsCommand(app, form(), "scaling-request-test");
  assert.deepEqual(command.spec.autoscaling, {minReplicas:2,maxReplicas:5,targetCPUUtilizationPercent:70});
  assert.equal(command.spec.replicas, 2);
  assert.equal(settingsCommand({...app,spec:{...app.spec,replicas:0}},form(),"scaling-stop-test").spec.replicas, 0);
});
await test("switch to fixed mode removes policy and invalid ranges fail before sending", () => {
  const active = {...app,spec:{...app.spec,autoscaling:{minReplicas:1,maxReplicas:4,targetCPUUtilizationPercent:70}}};
  const data = form(); data.set("scalingMode","fixed"); data.set("replicas","3");
  const command = settingsCommand(active,data,"fixed-request-test");
  assert.equal(command.spec.autoscaling, undefined); assert.equal(command.spec.replicas, 3);
  data.set("scalingMode","automatic"); data.set("maxReplicas","1");
  assert.throws(() => settingsCommand(app,data,"invalid-range-test"));
});
