import { expect, test } from "bun:test";
import { readJSON } from "../src/plugins/body";
test("JSON size limit applies to chunked requests without Content-Length", async () => {
  const request = new Request("http://localhost", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: new ReadableStream({
      start(c) {
        c.enqueue(new TextEncoder().encode('"' + "a".repeat(65_536) + '"'));
        c.close();
      },
    }),
  });
  await expect(readJSON(request, 65_536)).rejects.toHaveProperty("status", 413);
});
test("malformed JSON and unexpected media types are rejected", async () => {
  await expect(
    readJSON(
      new Request("http://localhost", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: "{",
      }),
      65_536,
    ),
  ).rejects.toHaveProperty("status", 400);
  await expect(
    readJSON(
      new Request("http://localhost", { method: "POST", body: "{}" }),
      65_536,
    ),
  ).rejects.toHaveProperty("status", 415);
});
