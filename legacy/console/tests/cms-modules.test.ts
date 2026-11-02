import test from "node:test";
import assert from "node:assert/strict";
import { cmsComponents, cmsHubURL, cmsModules, cmsRoleLabel } from "../src/lib/cms-modules";

test("demo CMS exposes its native editors and permission-aware modules", () => {
  for (const role of ["operator", "admin", "editor", "reader"]) {
    const modules = cmsModules("demo", role);
    assert.deepEqual(modules.map(module => module.id), ["content", "requests", "calendar"]);
    assert.ok(modules.every(module => module.writable === (role !== "reader")));
    assert.deepEqual(modules.flatMap(module => module.components), ["content-editor", "calendar-editor"]);
  }
  assert.equal(cmsRoleLabel("operator"), "Superadmin");
  assert.equal(cmsRoleLabel("admin"), "Tenant admin");
  assert.equal(cmsRoleLabel("editor"), "Editor");
  assert.equal(cmsRoleLabel("reader"), "Read-only");
});

test("shop management views restrict personal data and distinguish editable profiles from read-only orders", () => {
  for (const role of ["operator", "admin"]) {
    const modules = cmsModules("shop", role);
    assert.deepEqual(modules.map(module => module.id), ["products", "settings", "media", "orders", "customers"]);
    assert.deepEqual(modules.map(module => module.writable), [true, true, false, false, true]);
  }
  for (const role of ["editor", "reader"]) {
    const modules = cmsModules("shop", role);
    assert.deepEqual(modules.map(module => module.id), ["products", "settings", "media"]);
    assert.ok(modules.every(module => module.writable === (role === "editor" && module.id !== "media")));
    assert.deepEqual(modules.flatMap(module => module.components), ["product-variants-editor", "media-picker", "content-editor", "promotion-editor", "media-picker", "featured-products-editor", "media-picker"]);
  }
  for (const role of ["member", "owner", "", "superadmin"]) {
    assert.deepEqual(cmsModules("shop", role), []);
    assert.deepEqual(cmsModules("demo", role), []);
  }
});

test("public registry contains only module metadata and canonical local routes", () => {
  assert.equal(cmsHubURL("123"), "/sites/123");
  assert.equal(cmsHubURL("123/foreign"), "/sites/123%2Fforeign");
  const keys = ["id", "label", "description", "path", "kind", "components", "managementOnly", "readOnly", "writable"];
  for (const kind of ["demo", "shop"] as const) {
    for (const entry of cmsModules(kind, "operator")) {
      assert.ok(Object.keys(entry).every(key => keys.includes(key)));
      assert.ok(entry.path.startsWith("/") && !entry.path.startsWith("//"));
      for (const component of entry.components) assert.ok(cmsComponents[component]?.label);
    }
  }
});

test('retail can omit inherited promotion editor without changing the existing shop default',()=>{
 assert.ok(cmsModules('shop','operator').find(m=>m.id==='settings')?.components.includes('promotion-editor'));
 assert.equal(cmsModules('shop','operator',false,false).some(m=>m.components.includes('promotion-editor')),false);
});

test('hybrid site preserves content requests calendar and Canvas while adding shop modules',()=>{
 const modules=cmsModules('demo','operator',true,false,true);
 for(const id of ['builder','content','requests','calendar','products','settings','media','orders','customers'])assert.ok(modules.some(m=>m.id===id),id);
 assert.equal(new Set(modules.map(m=>m.id)).size,modules.length);
 assert.equal(modules.find(m=>m.id==='settings')?.label,'Shop settings');
 assert.deepEqual(modules.find(m=>m.id==='settings')?.components,['content-editor']);
 assert.equal(cmsModules('demo','editor',true,false,true).some(m=>['orders','customers'].includes(m.id)),false);
 assert.deepEqual(cmsModules('demo','operator').map(m=>m.id),['content','requests','calendar']);
});
