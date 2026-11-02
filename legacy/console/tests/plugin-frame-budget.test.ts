import { test } from "node:test";
import assert from "node:assert/strict";
import { createPluginFrameBudget } from "../src/lib/plugin-frame-budget";
test("long editor sessions keep bounded replay memory without exhausting the frame",()=>{
 const budget=createPluginFrameBudget();
 for(let i=0;i<2000;i++){assert.equal(budget.reserve(`chunk-${i}`,23000),true);budget.release(`chunk-${i}`);}
 assert.equal(budget.reserve("chunk-1999",23000),false);
 assert.equal(budget.reserve("next",23000),true);
});
test("concurrent calls, duplicate in-flight IDs and oversized frames are bounded",()=>{
 const budget=createPluginFrameBudget();
 for(let i=0;i<4;i++)assert.equal(budget.reserve(String(i),100),true);
 assert.equal(budget.reserve("4",100),false);
 assert.equal(budget.reserve("0",100),false);
 budget.release("0");assert.equal(budget.reserve("large",32769),false);
 assert.equal(budget.reserve("4",32768),true);
});
