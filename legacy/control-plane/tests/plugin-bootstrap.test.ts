import { test, expect } from "bun:test";
import { initializePluginState } from "../src/modules/plugins/bootstrap";
function fixture() {
  const inserts: unknown[][] = [];
  const connection = {
    async query(sql: string, values?: unknown[]) {
      if (sql.startsWith("INSERT INTO webdock_auth.plugin_installation")) inserts.push(values!);
      return { rows: [] };
    },
    release() {},
  };
  const db = { async connect() { return connection; } } as unknown as Parameters<typeof initializePluginState>[0];
  return { db, inserts };
}
test("fresh core bootstrap persists no product catalog", async () => {
  const {db,inserts}=fixture();
  await initializePluginState(db,{configVersion:1,modules:{}},"fresh");
  expect(inserts).toEqual([]);
});
test("offline legacy import retains the complete four-product catalog", async () => {
  const {db,inserts}=fixture();
  await initializePluginState(db,undefined,"legacy");
  expect(inserts.map(row=>row[0]).sort()).toEqual(["databases","hosting","mail","storage"]);
  expect(inserts.every(row=>row[1]==="enabled")).toBe(true);
});
test("reviewed fresh full selection enables only explicitly selected products", async () => {
  const {db,inserts}=fixture();
  await initializePluginState(db,{configVersion:1,modules:{mail:{enabled:true}}},"fresh");
  expect(inserts.map(row=>[row[0],row[1]]).sort()).toEqual([["databases","disabled"],["hosting","disabled"],["mail","enabled"],["storage","disabled"]]);
});
test("fresh bootstrap still requires a reviewed selection", async () => {
  await expect(initializePluginState(fixture().db,undefined,"fresh")).rejects.toThrow("explicit module selection");
});
