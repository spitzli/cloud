// Owned HTTP fixture for the real database workbench and installed Tabularis transport.
import { z } from "zod";
import { useState } from "react";
import { createRoot } from "react-dom/client";
import { I18nProvider } from "@spitzli/i18n/react";
import { Buffer } from "buffer/";
import { preloadTabularisWorkspace } from "@tabularis/web-ui";
import { DatabaseWorkbenchWithActions } from "@spitzli/module-databases/ui/workbench";
const result = z.union([z.object({ launch: z.object({ code: z.string(), gatewayOrigin: z.string() }) }), z.object({ error: z.string() })]);
const launch = async (id: string) => result.parse(await (await fetch(`/__launch/${id}`)).json());
const actions = { launchDatabase: launch, launchManagedDatabase: launch };
function Harness() {
  const [bindingID, setBindingID] = useState("first");
  const [visible, setVisible] = useState(true);
  const locale = new URLSearchParams(location.search).get("locale") === "de" ? "de" : "en";
  return <I18nProvider locale={locale} preference={locale}>
    <button onClick={() => { setBindingID("second"); }}>Switch database</button>
    <button onClick={() => { setVisible(false); }}>Leave database</button>
    {visible && <DatabaseWorkbenchWithActions bindingID={bindingID} userID="owned-fixture" actions={actions} />}
  </I18nProvider>;
}
// Match the production Databases loader before loading the retained geometry reader.
Object.assign(globalThis, { Buffer });
// Finish editor imports before mounting the connection-deadline fixture.
await preloadTabularisWorkspace();
const root = document.getElementById("root");
if (!root) throw new Error("Fixture root missing");
createRoot(root).render(<Harness />);
