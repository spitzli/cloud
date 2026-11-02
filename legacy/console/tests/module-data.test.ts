import { test } from "node:test";
import assert from "node:assert/strict";
import { optionalModuleData } from "../src/lib/module-data.ts";
test("minimal dashboard never contacts disabled products and does not report unavailable values as zero", async () => {
  let calls = 0;
  const load = async () => {
    calls++;
    return { bytes: 123 };
  };
  assert.equal(await optionalModuleData(undefined, "storage", load), null);
  assert.equal(
    await optionalModuleData(
      { revision: "1", modules: { storage: "disabled" } },
      "storage",
      load,
    ),
    null,
  );
  assert.equal(calls, 0);
  assert.deepEqual(
    await optionalModuleData(
      { revision: "2", modules: { storage: "draining" } },
      "storage",
      load,
    ),
    { bytes: 123 },
  );
  assert.equal(calls, 1);
  await assert.rejects(
    optionalModuleData(
      { revision: "3", modules: { storage: "enabled" } },
      "storage",
      async () => {
        throw Error("provider down");
      },
    ),
    /provider down/,
  );
});

test('allowances have no entry in an empty installation', async () => {
 const {hasProductModules}=await import('../src/lib/module-data');
 assert.equal(hasProductModules(undefined),false);
 assert.equal(hasProductModules({revision:'core',modules:{}}),false);
 assert.equal(hasProductModules({revision:'cms',modules:{cms:'draining'}}),true);
});
