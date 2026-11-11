import type { ReactNode } from "react";
import { ArrowClockwiseIcon } from "@phosphor-icons/react/dist/csr/ArrowClockwise";
import { PlugsIcon } from "@phosphor-icons/react/dist/csr/Plugs";
import { ApiError } from "./api";
import { ThemeButton } from "./theme";
import { LocalePicker } from "./Locale";
import { brandMark, button, cx } from "./ui";
// Shared frame for sign-in, account and full-page states, before any workspace shell exists.
export const authFrame = "relative flex min-h-dvh flex-col items-center justify-center gap-5 bg-page px-4 pb-16 pt-28 text-foreground";
// Global element rules sit outside Tailwind's layers, so spacing uses flex gaps rather than margins.
export const authPanel =
  "flex w-full max-w-[420px] flex-col gap-2 rounded-xl border border-outline bg-surface p-8 max-sm:p-6 " +
  "[&>p]:text-[13px] [&>p]:leading-relaxed [&>p]:text-muted " +
  "[&_.editor]:mt-5 [&_.text-button]:mx-auto [&_.text-button]:mt-3 [&_.text-button]:text-xs";
export function Brand({ product, href }: { product?: string | undefined; href?: string | undefined }) {
  const content = <>
    <span className={brandMark} aria-hidden><img src="/spitzli-logo.png" alt="" width={24} height={24} /></span>
    <span className="font-brand max-[420px]:sr-only">Spitzli Cloud</span>
    {product ? <span className="ml-1 border-l border-outline pl-3 text-[13px] font-normal tracking-normal text-muted max-[700px]:hidden">{product}</span> : null}
  </>;
  const style = "flex shrink-0 items-center gap-2 text-xl font-semibold tracking-tight text-foreground no-underline hover:no-underline max-[700px]:text-lg";
  return href ? <a className={style} href={href}>{content}</a> : <div className={style}>{content}</div>;
}
/** Account identity stays visible on phones and differs from workspace chrome. */
export function AccountBrand({ href = "/account" }: { href?: string }) {
  return <a href={href} className="account-brand" aria-label="Spitzli Account">
    <span className={brandMark} aria-hidden><img src="/spitzli-logo.png" alt="" width={24} height={24} /></span>
    <span><span className="font-brand">Account</span><span className="account-brand-label">by Spitzli</span></span>
  </a>;
}
export const accountFrame = `${authFrame} account-entry`;
export const accountPanel = `${authPanel} account-panel`;
export function AuthHeader({ product, href }: { product?: string | undefined; href?: string | undefined }) {
  return <header className="absolute inset-x-0 top-0 flex items-center justify-between px-6 py-5 max-sm:px-4">
    {product === "Account" ? <AccountBrand {...(href ? { href } : {})} /> : <Brand product={product} href={href} />}
    <div className="flex items-center gap-1">
      <LocalePicker />
      <ThemeButton />
    </div>
  </header>;
}
/** Network failures surface as browser-specific text; never show that raw to people. */
export function describeFailure(error: Error): string {
  if (error instanceof ApiError) return error.status >= 500 ? "The service is not responding right now." : error.message;
  if (error instanceof TypeError) return "Spitzli Cloud could not be reached. Check your connection and try again.";
  return error.message;
}
export function PageProgress({ product, children, action }: { product?: string | undefined; children: ReactNode; action?: ReactNode }) {
  return <main className={product === "Account" ? accountFrame : authFrame}>
    <AuthHeader product={product} />
    <div role="status" className="flex w-full max-w-[420px] flex-col items-center gap-5 text-center">
      <div className="h-1 w-40 overflow-hidden rounded-full bg-soft">
        <div className="h-full w-1/3 rounded-full bg-accent animate-[page-progress_1.1s_ease-in-out_infinite] motion-reduce:w-full motion-reduce:animate-none" />
      </div>
      <p className="text-[15px] font-medium">{children}</p>
      {action}
    </div>
  </main>;
}
export function PageFailure({ product, title = "Page unavailable", error, retry }: { product?: string | undefined; title?: string; error: Error; retry?: () => void }) {
  return <main className={product === "Account" ? accountFrame : authFrame}>
    <AuthHeader product={product} />
    <section role="alert" className={product === "Account" ? accountPanel : authPanel}>
      <span className="mb-3 inline-flex size-11 items-center justify-center rounded-full bg-danger-soft text-danger"><PlugsIcon size={22} /></span>
      <h1>{title}</h1>
      <p>{describeFailure(error)}</p>
      {retry ? <button className={cx(`${button("primary")} mt-5 self-start`)} onClick={retry}><ArrowClockwiseIcon />Try again</button> : null}
    </section>
  </main>;
}
