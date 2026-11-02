import test from "node:test";
import assert from "node:assert/strict";
import { createBridgeTransport } from "@webdock/cms-bridge/client";
import { MEDIA_BYTES } from "@webdock/cms-bridge/types";

test("large multipart keeps its media classification before Request body consumption", async () => {
  let began = false;
  const transport = createBridgeTransport(async ({ body }) => {
    assert.equal(body?.op, "begin");
    assert.equal(body?.kind, "media");
    began = true;
    throw Error("fixture stopped after classified begin");
  });
  const form = new FormData();
  form.set(
    "file",
    new File([new Uint8Array(MEDIA_BYTES)], "media.png", { type: "image/png" }),
  );
  await assert.rejects(
    transport("/api/cms", { method: "POST", body: form }),
    /fixture stopped after classified begin/,
  );
  assert.equal(began, true);
});
