import assert from "node:assert/strict";
import test from "node:test";
import { formText } from "../src/form";

await test("text form fields preserve strings and reject uploaded files", () => {
  const form = new FormData();
  form.set("name", "  Test  ");
  form.set("upload", new Blob(["data"]), "file.txt");
  assert.equal(formText(form, "name"), "  Test  ");
  assert.equal(formText(form, "missing"), "");
  assert.throws(() => formText(form, "upload"), /text form field/);
});
