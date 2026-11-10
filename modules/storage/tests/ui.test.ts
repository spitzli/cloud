import { test } from "node:test";
import assert from "node:assert/strict";
import { createElement as h } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { ModuleProvider } from "@spitzli/module-sdk/react";
import { I18nProvider } from "@spitzli/i18n/react";
import { StorageConnections } from "../src/ui/connections";
test("disabled storage renders no controls; draining retains refresh/removal without collecting credentials", () => {
  for (const state of ["disabled", "draining", "enabled"] as const) {
    const actions: string[] = [];
    const html = renderToStaticMarkup(h(I18nProvider, { locale: "en", preference: "en" },
      h(ModuleProvider, { view: { revision: state, modules: { storage: state } }, children:
        h(StorageConnections, { storage: { stores: [{ id: "123", label: "Existing", environment: "production", storeID: "456", prefix: "" }] },
          form: (action, label, children) => { actions.push(action); return h("form", { key: action }, label, children); } }) })));
    if (state === "disabled") { assert.equal(html, ""); assert.deepEqual(actions, []); }
    else {
      assert.ok(actions.includes("storage-refresh"));
      assert.ok(actions.includes("storage-remove"));
      assert.equal(actions.includes("storage-add"), state === "enabled");
      assert.equal(html.includes('type="password"'), state === "enabled");
    }
  }
});
