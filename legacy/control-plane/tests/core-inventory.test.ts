import {test, expect} from "bun:test";
import {assertCoreInventoryEmpty} from "../src/module-preflight";
test("core inventory rejects unverified RLS scopes before reading any inventory", async () => {
  for (const rows of [[],[{}],[{authorized:false}]]) {
    let queries=0;
    await expect(assertCoreInventoryEmpty({async query(){queries++;return {rows};}})).rejects.toThrow("verified installation preflight read authority");
    expect(queries).toBe(1);
  }
});
test("core inventory skips absent legacy tables after proving its read authority", async () => {
  await assertCoreInventoryEmpty({async query(sql){return {rows:[sql.includes(" AS authorized")?{authorized:true}:{relation:null}]};}});
});
