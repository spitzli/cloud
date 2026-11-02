import test from "node:test";
import assert from "node:assert/strict";
import { createElement as h } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nProvider } from "@webdock/i18n/react";
import { ManagedDatabaseRows } from "../src/components/databases/managed-rows";

test("managed databases are visible without manual bindings and link ready authorized databases to Tabularis", () => {
  const html = renderToStaticMarkup(h(I18nProvider, { locale: "en", preference: "en" },
    h(ManagedDatabaseRows, { databases: [
      { id: "101988493768523776", projectID: "10", name: "Orders", state: "ready", canOpen: true },
      { id: "21", projectID: "10", name: "Paused", state: "paused", canOpen: false },
      { id: "22", projectID: "10", name: "Member only", state: "ready", canOpen: false },
    ] })));
  assert.match(html, /Orders/);
  assert.match(html, /href="\/hosting\/databases\/101988493768523776\/viewer"/);
  assert.match(html, /Open in Tabularis/);
  assert.doesNotMatch(html, /\/databases\/21\/viewer|\/databases\/22\/viewer/);
  for (const id of ["101988493768523776", "21", "22"]) assert.match(html, new RegExp(`href="/hosting/databases/${id}"`));
});
