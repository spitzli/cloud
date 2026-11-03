// Tailwind recipes shared by every Console surface. Compose with cx(recipe, "extra utilities"):
// later classes win, so call sites can override a recipe. Colours come from the theme tokens in styles.css.
import { twMerge } from "tailwind-merge";
/** Joins class lists; a later utility replaces an earlier one of the same kind (w-full → w-auto). */
export const cx = (...classes: (string | false | null | undefined)[]) => twMerge(classes);
const buttonBase =
  "inline-flex min-h-9 w-fit max-w-full items-center justify-center gap-1.5 whitespace-nowrap rounded-md border px-3 text-sm max-sm:min-h-11 max-sm:whitespace-normal font-medium no-underline transition-colors " +
  "hover:no-underline disabled:cursor-not-allowed disabled:opacity-55 [&_svg]:size-4 [&_svg]:shrink-0";
const buttonVariants = {
  secondary: "border-outline-strong bg-surface text-foreground enabled:hover:bg-soft",
  primary: "border-foreground bg-foreground text-surface enabled:hover:opacity-90",
  quiet: "border-transparent bg-transparent text-foreground enabled:hover:bg-soft",
  danger: "border-danger bg-danger text-on-accent enabled:hover:opacity-90",
};
/** Buttons and links share compact sizing; icon targets are 36px desktop, 44px mobile. */
export function button(variant: keyof typeof buttonVariants = "secondary", size?: "icon") {
  return cx(buttonBase, buttonVariants[variant], size === "icon" && "w-9 px-0 max-sm:w-11");
}
/** Text inputs, selects and textareas. */
export const input =
  "min-h-9 w-full max-sm:min-h-11 min-w-0 rounded-md border border-outline-strong bg-surface px-3 py-2 text-sm max-sm:text-base font-normal text-foreground placeholder:text-muted " +
  "focus-visible:border-focus focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring [&:is(input,textarea):read-only]:bg-soft disabled:cursor-not-allowed disabled:opacity-55 user-invalid:border-danger";
/** A label wrapping its control. */
export const field = "grid gap-1.5 text-[13px] font-medium";
export const hint = "text-xs font-normal text-muted";
export const card = "overflow-hidden rounded-lg border border-outline bg-surface";
export const cardBody = "grid gap-4 p-5 max-sm:p-4";
/** Shared spacing for every native plugin resource view. */
export const resourceGrid = "grid gap-4";
/** Resource menus anchor left on stacked mobile headings and never exceed the viewport. */
export const actionMenu = cx(card, "absolute right-0 z-20 mt-2 w-80 max-w-[calc(100vw-2rem)] p-1.5 shadow-lg max-sm:right-auto max-sm:left-0");
/** Quiet bottom bar of a card: hint on the left, action on the right. */
export const cardFooter =
  "flex min-h-13 flex-wrap items-center justify-between gap-3 border-t border-outline bg-subtle px-5 py-3 text-[13px] text-muted max-sm:px-4";
/** Rows of a card list, separated by hairlines. */
export const cardRow = "flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-t border-outline px-5 py-3 text-sm max-sm:px-4";
export const pageHeading = "mb-6 flex min-w-0 items-end justify-between gap-4 [&>div]:min-w-0 max-sm:mb-5 max-sm:flex-col max-sm:items-start max-sm:gap-4";
export const pageTitle = "text-[26px] font-semibold leading-tight tracking-[-0.025em] [overflow-wrap:anywhere] max-sm:text-2xl";
export const pageLead = "mt-1.5 max-w-[76ch] text-sm text-muted";
export const sectionTitle = "text-[15px] font-semibold tracking-[-0.01em]";
/** Tables sit in a rounded, horizontally scrollable frame. */
export const tableFrame = "overflow-x-auto rounded-lg border border-outline bg-surface";
export const table = "w-full border-collapse text-left text-sm";
export const th = "border-b border-outline bg-subtle px-4 py-2.5 text-xs font-medium text-muted max-sm:px-3.5";
export const td = "border-b border-outline px-4 py-3 [overflow-wrap:anywhere] group-last:border-b-0 max-sm:px-3.5";
export const tr = "group relative transition-colors hover:bg-subtle";
/** Makes a row's primary link cover the whole row (table row or card row). Other controls in the
 *  row need `relative z-10` to stay clickable on top of it. */
export const rowLink = "after:absolute after:inset-0 after:content-[''] focus-visible:outline-none focus-visible:after:rounded-[inherit] focus-visible:after:ring-2 focus-visible:after:ring-focus";
/** A card row that is clickable as a whole; pair with rowLink on its primary link. */
export const clickableRow = "relative cursor-pointer transition-colors hover:bg-subtle";
const tones = { neutral: "before:bg-muted", success: "before:bg-success", danger: "before:bg-danger", warning: "before:bg-warning", accent: "before:bg-accent" };
/** Status pill with a coloured dot. */
export function badge(tone: keyof typeof tones = "neutral") {
  return `inline-flex items-center gap-1.5 whitespace-nowrap rounded-full border border-outline bg-surface px-2 text-xs font-medium leading-5 capitalize text-foreground before:size-[7px] before:shrink-0 before:rounded-full before:content-[''] ${tones[tone]}`;
}
/** Maps lifecycle words to a badge tone. */
export function statusTone(status: string | undefined): keyof typeof tones {
  if (!status) return "neutral";
  if (/^(active|ready|healthy|running|succeeded|enabled|verified|attached|available)$/i.test(status)) return "success";
  if (/^(failed|error|needs-reconciliation|disabled|revoked|unavailable)$/i.test(status)) return "danger";
  if (/^(pending|queued|provisioning|starting|stopping|draining|degraded)$/i.test(status)) return "warning";
  return "neutral";
}
export const notice = "rounded-lg border border-outline bg-subtle px-4 py-3 text-[13px]";
export const mono = "font-mono text-xs";
/** Keep the owned white/violet mark legible in both themes. */
export const brandMark =
  "inline-flex size-8 shrink-0 items-center justify-center rounded-md bg-logo p-1";
export const backLink = "mb-3 inline-flex items-center gap-1.5 text-[13px] text-muted hover:text-foreground hover:no-underline";
export const legend = "px-0 pb-2 text-sm font-semibold";
/** Underlined inline action, e.g. "Use a recovery code instead". */
export const textButton = "mx-auto mt-3 border-0 bg-transparent py-2 text-xs text-foreground underline underline-offset-[3px]";
