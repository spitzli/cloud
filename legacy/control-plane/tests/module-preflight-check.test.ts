import {test, expect} from "bun:test";
import {assertModuleCanDisable} from "../src/module-preflight-check";
const checks = [["webdock_auth.example", "status <> 'deleted'"]] as const;
function reader(rows: Record<string, unknown>[]) {
 const queries: string[] = [];
 return {queries, async query(sql: string) {queries.push(sql); return {rows: rows.length ? [rows.shift()!] : []};}};
}
test("fresh core can start without product tables", async () => {
 const db = reader([{relation: null}]);
 await assertModuleCanDisable(db, "example", checks);
 expect(db.queries).toHaveLength(1);
});
test("existing products still require preflight authority and no retained resources", async () => {
 for (const rows of [[], [{}], [{relation: "example"}, {present: false}], [{relation: "example"}, {present: true}, {retained: true}], [{relation: "example"}, {present: true}]])
   await expect(assertModuleCanDisable(reader(rows), "example", checks)).rejects.toThrow();
 const db = reader([{relation: "example"}, {present: true}, {retained: false}]);
 await assertModuleCanDisable(db, "example", checks);
 expect(db.queries).toHaveLength(3);
});
