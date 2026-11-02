import { CheckIcon } from "@phosphor-icons/react/dist/csr/Check";
import type { PluginView } from "./views";
import * as ui from "../ui";

type Offering = PluginView["manifest"]["offerings"][number];
/** What each offering of a plugin is and brings, from the plugin's own manifest. */
export function Offerings({ offerings, create }: { offerings: Offering[]; create?: (offering: string) => (() => void) | undefined }) {
  return (
    <ul className={ui.cx("grid gap-4", offerings.length > 1 ? "md:grid-cols-2" : "")} aria-label="Options">
      {offerings.map((offering) => {
        const start = create?.(offering.id);
        return (
          <li key={offering.id} className={ui.cx(ui.card, ui.cardBody, "content-start gap-3")}>
            <h3 className={ui.sectionTitle}>{offering.label}</h3>
            {offering.description ? <p className="text-sm text-muted">{offering.description}</p> : null}
            {offering.highlights?.length ? (
              <ul className="grid gap-1.5 text-sm">
                {offering.highlights.map((highlight) => (
                  <li key={highlight} className="flex gap-2">
                    <CheckIcon size={16} className="mt-0.5 shrink-0 text-success" />
                    {highlight}
                  </li>
                ))}
              </ul>
            ) : null}
            {start ? <div><button type="button" className={ui.button("primary")} onClick={start}>Create {offering.label.toLowerCase()}</button></div> : null}
          </li>
        );
      })}
    </ul>
  );
}
