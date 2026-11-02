import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import ts from "typescript";
import { translator } from "@webdock/i18n";
test("Git deployment errors remain actionable in English and German", () => {
  const directory = "../control-plane/src/modules/hosting";
  const files = readdirSync(directory)
    .filter((p) => /^git.*\.ts$/.test(p))
    .map((p) => directory + "/" + p)
    .concat(["src/lib/hosting-actions.ts"]);
  const messages = new Set<string>();
  for (const file of files) {
    const source = ts.createSourceFile(
      file,
      readFileSync(file, "utf8"),
      ts.ScriptTarget.Latest,
      true,
    );
    function literal(node: ts.Node) {
      if (ts.isStringLiteral(node)) messages.add(node.text);
      else if (ts.isConditionalExpression(node)) {
        literal(node.whenTrue);
        literal(node.whenFalse);
      }
    }
    function visit(node: ts.Node) {
      if (
        ts.isNewExpression(node) &&
        node.expression.getText(source) === "HostingError" &&
        node.arguments?.[1]
      )
        literal(node.arguments[1]);
      if (
        ts.isCallExpression(node) &&
        ["missing", "unavailable", "denied"].includes(
          node.expression.getText(source),
        ) &&
        node.arguments[0]
      )
        literal(node.arguments[0]);
      ts.forEachChild(node, visit);
    }
    visit(source);
  }
  for (const message of messages) {
    assert.equal(translator("en").error(message), message, message);
    assert.notEqual(translator("de").error(message), message, message);
  }
});
test("Git target actions and dynamic statuses have German presentation labels", () => {
  for (const message of [
    "Bind Vercel target",
    "Select an existing authorized Vercel project. Disable native Git builds and configure Frankfurt Functions before binding.",
    "Verify and bind target",
    "Vercel project ID",
    "Customer Vercel connection",
    "Platform Vercel connection",
    "Renew approval",
    "active",
    "disconnected",
    "revalidation-required",
    "queued",
    "running",
    "succeeded",
    "failed",
    "cancelled",
    "awaiting-approval",
    "deploying",
    "ready",
    "superseded",
    "needs-reconciliation",
  ])
    assert.notEqual(translator("de").t(message), message, message);
});
