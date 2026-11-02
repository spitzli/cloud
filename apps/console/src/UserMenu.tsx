import { useId, type KeyboardEvent, type ToggleEvent } from "react";
import { ShieldCheckIcon } from "@phosphor-icons/react/dist/csr/ShieldCheck";
import { SignOutIcon } from "@phosphor-icons/react/dist/csr/SignOut";
import { SquaresFourIcon } from "@phosphor-icons/react/dist/csr/SquaresFour";
/** Places a native popover below its trigger; CSS anchor positioning is not available in every browser. */
export function alignBelowTrigger(event: ToggleEvent<HTMLElement>) {
  const trigger = event.currentTarget.previousElementSibling?.getBoundingClientRect();
  if (event.newState !== "open" || !trigger) return;
  event.currentTarget.style.top = `${String(trigger.bottom + 6)}px`;
  event.currentTarget.style.right = `${String(Math.max(8, innerWidth - trigger.right))}px`;
}
export const menuPanel = "fixed m-0 w-64 rounded-lg border border-outline bg-surface p-1 text-foreground shadow-lg [inset:auto]";
export const menuItem = "flex w-full items-center gap-2.5 rounded-md px-3 py-2 text-left text-[13px] text-foreground no-underline hover:bg-soft hover:no-underline focus-visible:bg-soft focus-visible:outline-none";
function menuKeyboard(event: KeyboardEvent<HTMLElement>) {
  const items = [...event.currentTarget.querySelectorAll<HTMLElement>('[role="menuitem"]:not(:disabled)')];
  if (!items.length) return;
  let index = items.findIndex((item) => item === document.activeElement);
  switch (event.key) {
    case "ArrowDown": index = (index + 1) % items.length; break;
    case "ArrowUp": index = (index <= 0 ? items.length : index) - 1; break;
    case "Home": index = 0; break;
    case "End": index = items.length - 1; break;
    default: return;
  }
  event.preventDefault();
  items[index]?.focus();
}
/** The same account menu in Console and on the account surface, so both feel like one application. */
export function UserMenu({ name, email, consoleHref, accountHref, signOut }: {
  name: string; email: string; consoleHref?: string | undefined; accountHref?: string | undefined; signOut: (() => void) | string;
}) {
  const id = useId();
  return <div className="relative">
    <button className="ml-1.5 grid size-8 cursor-pointer place-items-center rounded-full border border-outline bg-soft text-xs font-semibold text-foreground" popoverTarget={id} aria-label={`Account menu for ${name}`}>{name.slice(0, 1).toUpperCase()}</button>
    <div id={id} popover="auto" role="menu" aria-label="Account" onKeyDown={menuKeyboard} onToggle={(event) => {
      alignBelowTrigger(event);
      if (event.newState === "open") event.currentTarget.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    }} className={menuPanel}>
      <div className="flex flex-col px-3 pb-2 pt-2.5">
        <strong className="truncate text-[13px]">{name}</strong>
        <span className="truncate text-xs text-muted">{email}</span>
      </div>
      <div className="my-1 border-t border-outline" />
      {consoleHref ? <a role="menuitem" className={menuItem} href={consoleHref}><SquaresFourIcon size={17} aria-hidden />Open Console</a> : null}
      {accountHref ? <a role="menuitem" className={menuItem} href={accountHref}><ShieldCheckIcon size={17} aria-hidden />Account security</a> : null}
      {typeof signOut === "string"
        ? <a role="menuitem" className={menuItem} href={signOut}><SignOutIcon size={17} aria-hidden />Sign out</a>
        : <button role="menuitem" className={menuItem} onClick={signOut}><SignOutIcon size={17} aria-hidden />Sign out</button>}
    </div>
  </div>;
}
