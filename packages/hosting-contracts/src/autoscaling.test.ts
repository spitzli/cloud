import { test, expect } from "bun:test";
import { appSpecSchema, preparedAppSpecSchema } from "./app-spec";
import { appDemand, commandSchema, readActions } from "./index";

const spec = { template: "custom", image: `registry.example/app@sha256:${"a".repeat(64)}`, args: [], port: 8080, healthPath: "/", cpuMillicores: 100, memoryBytes: 67108864, ephemeralBytes: 67108864, replicas: 2, volumeBytes: 0 };
const policy = { minReplicas: 2, maxReplicas: 5, targetCPUUtilizationPercent: 70 };

test("client demand matches server maximum reservation and stopped lifecycle", () => {
  const automatic = appSpecSchema.parse({...spec, autoscaling:policy});
  expect(appDemand(automatic).cpuMillicores).toBe(500);
  expect(appDemand(automatic).replicasPerApp).toBe(5);
  expect(appDemand({...automatic,replicas:0}).cpuMillicores).toBe(0);
});

test("native inspection and metrics use the strict read contract", () => {
  for (const action of ["apps.inspect", "apps.metrics"] as const) {
    const value = commandSchema.parse({action, appID:"301", idempotencyKey:"inspect-scaling-test"});
    expect(readActions.has(value.action)).toBe(true);
    expect(commandSchema.safeParse({...value, operator:true}).success).toBe(false);
  }
});

test("automatic application policy preserves fixed compatibility and survives stop", () => {
  expect(appSpecSchema.parse(spec).autoscaling).toBeUndefined();
  expect(appSpecSchema.parse({ ...spec, autoscaling: policy }).autoscaling).toEqual(policy);
  expect(appSpecSchema.parse({ ...spec, replicas: 0, autoscaling: policy }).replicas).toBe(0);
  const { image: _image, template: _template, ...prepared } = spec;
  expect(preparedAppSpecSchema.parse({ ...prepared, autoscaling: policy }).autoscaling).toEqual(policy);
});

test("automatic policies reject unsupported and contradictory resource bounds", () => {
  for (const autoscaling of [null, { ...policy, minReplicas: 0 }, { ...policy, minReplicas: 6 }, { ...policy, maxReplicas: 21 }, { ...policy, maxReplicas: 2.5 }, { ...policy, targetCPUUtilizationPercent: 9 }, { ...policy, targetCPUUtilizationPercent: 91 }, { ...policy, arbitrary: true }]) {
    expect(appSpecSchema.safeParse({ ...spec, autoscaling }).success).toBe(false);
  }
  expect(appSpecSchema.safeParse({ ...spec, replicas: 1, autoscaling: policy }).success).toBe(false);
  expect(appSpecSchema.safeParse({ ...spec, volumeBytes: 67108864, autoscaling: policy }).success).toBe(false);
});
