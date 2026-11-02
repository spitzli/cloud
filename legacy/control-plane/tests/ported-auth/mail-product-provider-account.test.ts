import assert from "node:assert/strict";
import test from "node:test";
import { verifyTurboSendingAccount } from "../../src/modules/mail/product/provider-account";
test("BYOK identity uses exact account ID and a nondelivering permission probe", async () => {
  const calls: { url: string; body: unknown }[] = [];
  const pair = {
    consumerKey: "key",
    consumerSecret: "secret",
    region: "eu" as const,
  };
  const fetcher: typeof fetch = async (url, init) => {
    calls.push({ url: String(url), body: init?.body });
    return String(url).endsWith("/user/info")
      ? new Response('{"id":26191165,"active":"1"}')
      : new Response('{"message":"error"}', { status: 400 });
  };
  assert.deepEqual(await verifyTurboSendingAccount(pair, fetcher), {
    accountID: "26191165",
  });
  assert.equal(calls[1].body, "{}");
  assert.equal(calls[1].url, "https://api.eu.turbo-smtp.com/api/v2/mail/send");
  await assert.rejects(
    verifyTurboSendingAccount(pair, async (url) =>
      String(url).endsWith("/user/info")
        ? new Response('{"id":"26191165","active":"1"}')
        : new Response("{}", { status: 401 }),
    ),
    { code: "sending_permission_required" },
  );
  await assert.rejects(
    verifyTurboSendingAccount(
      pair,
      async () => new Response('{"id":9007199254740993123,"active":false}'),
    ),
  );
});
