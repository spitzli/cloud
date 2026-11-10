import { configureFullInstallation } from "../src/full-installation";
configureFullInstallation();
import { test, expect } from "bun:test";
import {
  loadProductOperation,
  type ProductOperation,
} from "../src/modules/portal/product-operation";
const runtime: ProductOperation = {
  async dispatch() {
    return null;
  },
  publicError() {
    return undefined;
  },
};
test("core calls and disabled products do not load product implementations", async () => {
  const previous = process.env.SPITZLI_MODULES;
  process.env.SPITZLI_MODULES = JSON.stringify({
    configVersion: 1,
    modules: { mail: { enabled: false } },
  });
  const loaded: string[] = [];
  const loader = async (module: string) => {
    loaded.push(module);
    return runtime;
  };
  try {
    expect(await loadProductOperation("session", [], loader)).toBeUndefined();
    await expect(
      loadProductOperation("mailConnections", ["12"], loader),
    ).rejects.toHaveProperty("code", "module_disabled");
    await expect(
      loadProductOperation("getTenantMail", ["12"], loader),
    ).rejects.toHaveProperty("code", "module_disabled");
    expect(loaded).toEqual([]);
    process.env.SPITZLI_MODULES = JSON.stringify({
      configVersion: 1,
      modules: { mail: { enabled: true } },
    });
    expect(await loadProductOperation("mailConnections", ["12"], loader)).toBe(
      runtime,
    );
    expect(loaded).toEqual(["mail"]);
  } finally {
    if (previous === undefined) delete process.env.SPITZLI_MODULES;
    else process.env.SPITZLI_MODULES = previous;
  }
});
