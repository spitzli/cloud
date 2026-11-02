import { test, expect } from "bun:test";
import "./helpers/no-network";
test("the regression harness rejects accidental provider traffic before a request is sent", async () => {
  await expect(
    fetch("https://sandbox-api.polar.sh/v1/organizations"),
  ).rejects.toThrow("External network disabled");
});
