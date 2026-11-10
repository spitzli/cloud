import { demoAccess } from "./demo-bridges";
import { getStudioSession } from "../portal/request-context";
import { createCMSCanvasBridge } from "@spitzli/module-cms/server/canvas-bridge";
export { type CanvasCommand } from "@spitzli/module-cms/server/canvas-bridge";
export const { canvasList, canvasPage, writeCanvas } = createCMSCanvasBridge({
  demoAccess,
  getStudioSession,
  fetch: (input, init) => globalThis.fetch(input, init),
});
