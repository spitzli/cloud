import { test, expect } from "bun:test";
import { setupProductAvailable } from "../src/modules/hosting/product-admission";
test("setup options respect product installation state and API process ownership", () => {
  const states = {
    WEBDOCK_MODULES: JSON.stringify({
      configVersion: 1,
      modules: {
        hosting: { enabled: true },
        mail: { state: "draining" },
        databases: { enabled: false },
      },
    }),
  };
  expect(setupProductAvailable("mail", states)).toBe(true);
  expect(setupProductAvailable("databases", states)).toBe(false);
  expect(
    setupProductAvailable("mail", {
      ...states,
      WEBDOCK_API_DOMAINS: "auth,hosting",
    }),
  ).toBe(false);
  expect(
    setupProductAvailable("databases", { WEBDOCK_API_DOMAINS: "auth,hosting" }),
  ).toBe(false);
});
