import { expect, test } from "bun:test";
import panel from "./fixtures/servers-panel.json";
import { panelSchema } from "../../../apps/console/src/plugins/contracts";

test("the exact compute subprocess panel conforms to the current Console contract", () => {
  expect(panelSchema.parse(panel)).toEqual(panel);
});
