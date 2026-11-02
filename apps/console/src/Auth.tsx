import { formText } from "./form";
import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { useApi, useSession } from "./context";
import { AuthHeader, PageProgress, authFrame, authPanel } from "./AuthFrame";
import * as ui from "./ui";
const panelForm = "mt-5 grid gap-5";
const authResult = z.object({
  twoFactorRedirect: z.boolean().optional(),
  status: z.boolean().optional(),
});
export function SignIn() {
  const api = useApi();
  const client = useQueryClient();
  const [mode, setMode] = useState<"signin" | "totp" | "reset">("signin");
  const [message, setMessage] = useState("");
  const login = useMutation({
    mutationFn: async (data: FormData) => {
      if (mode === "reset")
        return api.write(
          "/api/auth/request-password-reset",
          "POST",
          {
            email: data.get("email"),
            redirectTo: new URL("/reset-password", window.location.origin).href,
          },
          authResult,
          true,
        );
      if (mode === "totp")
        return api.write(
          "/api/auth/two-factor/verify-totp",
          "POST",
          { code: data.get("code") },
          authResult,
          true,
        );
      return api.write(
        "/api/auth/sign-in/email",
        "POST",
        { email: data.get("email"), password: data.get("password") },
        authResult,
        true,
      );
    },
    onSuccess: (result) => {
      if (mode === "reset") {
        setMessage(
          "If an account matches this email, a reset link will be sent.",
        );
        return;
      }
      if (result.twoFactorRedirect) {
        setMode("totp");
        return;
      }
      client.clear();
      window.location.assign("/projects");
    },
  });
  if (api.config.sso) return <SignInRedirect />;
  return (
    <main className={authFrame}>
      <AuthHeader product="Console" />
      <section className={authPanel}>
        <h1>
          {mode === "totp"
            ? "Verify your identity"
            : mode === "reset"
              ? "Reset your password"
              : "Welcome back"}
        </h1>
        <p>
          {mode === "totp"
            ? "Enter the code from your authenticator."
            : mode === "reset"
              ? "We’ll send a reset link to your account email."
              : "Sign in to your Spitzli Cloud account."}
        </p>
        {/* A new form per step: password managers must not reuse the email field for the code. */}
        <form
          key={mode}
          className={panelForm}
          onSubmit={(event) => {
            event.preventDefault();
            login.mutate(new FormData(event.currentTarget));
          }}
        >
          {mode === "totp" ? (
            <label className={ui.field}>
              Authentication code
              <input
                className={ui.input}
                name="code"
                inputMode="numeric"
                pattern="[0-9]{6}"
                autoComplete="one-time-code"
                required
                autoFocus
              />
            </label>
          ) : (
            <label className={ui.field}>
              Email
              <input
                className={ui.input}
                type="email"
                name="email"
                autoComplete="username"
                required
                maxLength={254}
                autoFocus
              />
            </label>
          )}
          {mode === "signin" ? (
            <label className={ui.field}>
              Password
              <input
                className={ui.input}
                type="password"
                name="password"
                autoComplete="current-password"
                required
                maxLength={1024}
              />
            </label>
          ) : null}
          {login.isError ? (
            <p role="alert" className="text-sm text-danger">
              {login.error.message}
            </p>
          ) : null}
          {message ? (
            <p role="status" className="text-sm text-success">
              {message}
            </p>
          ) : null}
          <button className={ui.cx(`${ui.button("primary")} w-full`)} disabled={login.isPending}>
            {login.isPending
              ? "Working…"
              : mode === "signin"
                ? "Sign in"
                : mode === "totp"
                  ? "Verify code"
                  : "Send reset link"}
          </button>
        </form>
        {mode !== "totp" ? (
          <button
            className={ui.textButton}
            onClick={() => {
              login.reset();
              setMessage("");
              setMode(mode === "signin" ? "reset" : "signin");
            }}
          >
            {mode === "signin" ? "Forgot password?" : "Back to sign in"}
          </button>
        ) : null}
      </section>
    </main>
  );
}
export function ResetPassword() {
  const api = useApi();
  const [token] = useState(() =>
    new URLSearchParams(window.location.search).get("token"),
  );
  const reset = useMutation({
    mutationFn: (newPassword: string) =>
      api.write(
        "/api/auth/reset-password",
        "POST",
        { token, newPassword },
        authResult,
        true,
      ),
  });
  return (
    <main className={authFrame}>
      <AuthHeader product="Console" />
      <section className={authPanel}>
        <h1>Set a new password</h1>
        {!token ? (
          <p role="alert">
            This reset link is invalid. Request a new one from the sign-in page.
          </p>
        ) : reset.isSuccess ? (
          <p role="status">
            Password updated. <a href="/">Sign in</a>
          </p>
        ) : (
          <form
            className={panelForm}
            onSubmit={(event) => {
              event.preventDefault();
              reset.mutate(
                formText(new FormData(event.currentTarget), "newPassword"),
              );
            }}
          >
            <label className={ui.field}>
              New password
              <input
                className={ui.input}
                name="newPassword"
                type="password"
                autoComplete="new-password"
                minLength={12}
                maxLength={1024}
                required
              />
            </label>
            {reset.isError ? (
              <p role="alert" className="text-sm text-danger">
                {reset.error.message}
              </p>
            ) : null}
            <button className={ui.cx(`${ui.button("primary")} w-full`)} disabled={reset.isPending}>
              {reset.isPending ? "Updating…" : "Update password"}
            </button>
          </form>
        )}
      </section>
    </main>
  );
}
export function Account() {
  const session = useSession();
  const api = useApi();
  const client = useQueryClient();
  const change = useMutation({
    mutationFn: (data: FormData) =>
      api.write(
        "/api/auth/change-password",
        "POST",
        {
          currentPassword: data.get("currentPassword"),
          newPassword: data.get("newPassword"),
          revokeOtherSessions: true,
        },
        z.object({}),
        true,
      ),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["session"] });
    },
  });
  if (api.config.sso)
    return (
      <section>
        <div className={ui.pageHeading}>
          <div>
            <h1 className={ui.pageTitle}>Account</h1>
            <p className={ui.pageLead}>{session.user.email}</p>
          </div>
        </div>
        <section className={ui.card}>
          <div className={ui.cardBody}>
            <h2 className={ui.sectionTitle}>Account security</h2>
            <p className="text-muted">
              Manage your password, authenticator, passkeys, and sessions in your
              Spitzli Cloud account.
            </p>
          </div>
          <div className={ui.cx(`${ui.cardFooter} justify-end`)}>
          <a
            className={ui.button("primary")}
            href={new URL("/account", api.config.authOrigin).href}
          >
            Open account security
          </a>
          </div>
        </section>
      </section>
    );
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.pageTitle}>Account</h1>
          <p className={ui.pageLead}>Manage your sign-in and account security.</p>
        </div>
      </div>
      <div className="grid items-start gap-8 lg:grid-cols-[minmax(0,1fr)_300px]">
        <form
          className={ui.card}
          onSubmit={(event) => {
            event.preventDefault();
            const form = event.currentTarget;
            change.mutate(new FormData(form), {
              onSuccess: () => {
                form.reset();
              },
            });
          }}
        >
          <div className={ui.cx(`${ui.cardBody} gap-5`)}>
          <h2 className={ui.sectionTitle}>Change password</h2>
          {session.user.mustChangePassword ? (
            <p role="alert" className={ui.notice}>Update your password before continuing.</p>
          ) : null}
          <label className={ui.field}>
            Current password
            <input
                className={ui.input}
              name="currentPassword"
              type="password"
              required
              autoComplete="current-password"
              maxLength={1024}
            />
          </label>
          <label className={ui.field}>
            New password
            <input
                className={ui.input}
              name="newPassword"
              type="password"
              required
              autoComplete="new-password"
              minLength={12}
              maxLength={1024}
            />
          </label>
          {change.isError ? (
            <p role="alert" className="text-sm text-danger">
              {change.error.message}
            </p>
          ) : null}
          {change.isSuccess ? (
            <p role="status" className="text-sm text-success">
              Password updated.
            </p>
          ) : null}
          </div>
          <div className={ui.cardFooter}>
            <span>Other sessions will be signed out.</span>
            <button className={ui.button("primary")} disabled={change.isPending}>
              {change.isPending ? "Updating…" : "Update password"}
            </button>
          </div>
        </form>
        <aside className="grid gap-1 text-[13px] [overflow-wrap:anywhere]">
          <h2 className="text-sm font-semibold">{session.user.name}</h2>
          <p className="text-muted">{session.user.email}</p>
          <dl className="mt-4 grid gap-4 [&_dd]:mt-1 [&_dd]:text-sm [&_dt]:text-xs [&_dt]:text-muted">
            <div>
              <dt>Email</dt>
              <dd>
                {session.user.emailVerified ? "Verified" : "Not verified"}
              </dd>
            </div>
            <div>
              <dt>Two-factor authentication</dt>
              <dd>
                {session.user.twoFactorEnabled ? "Enabled" : "Not enabled"}
              </dd>
            </div>
            <div>
              <dt>Current session expires</dt>
              <dd>{new Date(session.session.expiresAt).toLocaleString()}</dd>
            </div>
          </dl>
        </aside>
      </div>
    </section>
  );
}
/** Console sign-in happens on the account origin; go there directly unless the last attempt failed. */
function SignInRedirect() {
  const failed = new URLSearchParams(window.location.search).has("error");
  const returnTo = window.location.pathname === "/login" ? "/projects" : window.location.pathname + window.location.search;
  const href = `/api/sso/login?returnTo=${encodeURIComponent(returnTo)}`;
  useEffect(() => {
    if (!failed) window.location.replace(href);
  }, [failed, href]);
  if (!failed)
    return <PageProgress product="Console" action={<a className="text-xs text-muted underline-offset-4 hover:underline" href={href}>Continue to sign in</a>}>
      Redirecting to sign in…
    </PageProgress>;
  return <main className={authFrame}>
    <AuthHeader product="Console" />
    <section className={authPanel} role="alert">
      <h1>Sign-in was not completed</h1>
      <p>Your sign-in request expired or was cancelled. Start again to open your workspace.</p>
      <a className={ui.cx(`${ui.button("primary")} mt-5 w-full`)} href={href}>Sign in again</a>
    </section>
  </main>;
}
