import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import ts from "typescript";
import { translator } from "@webdock/i18n";
test("Setup and service errors remain actionable in English and German", () => {
  const directory = "../control-plane/src/modules/hosting";
  const files = readdirSync(directory)
    .filter((p) => /^(setup-.*|project-setup|service-bindings|database-binding|mail-binding|binding-runtime)\.ts$/.test(p))
    .map((p) => directory + "/" + p)
    .concat(["src/lib/project-setup-actions.ts"]);
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
