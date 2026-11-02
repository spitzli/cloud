"use client";
import { useI18n } from "@webdock/i18n/react";
/* eslint-disable @next/next/no-location-assign-relative-destination -- Authentication transitions reload the page to synchronize session cookies and the provider's signed query. */

import { useState, type FormEvent, type ReactNode } from "react";
import Link from "next/link";
import { accountClient, authClient, goToAccount } from "./auth-client";
import { Passkeys } from "./passkeys";
import { signInError, SIGN_IN_EXPIRED } from "../lib/sign-in-recovery";

function Notice({
  message,
  error = false,
}: {
  message: string;
  error?: boolean;
}) {
 const { t, error: translateError } = useI18n();
  return message ? (
    <p
      className={error ? "notice error" : "notice"}
      role={error ? "alert" : "status"}
    >
      {error ? translateError(message) : t(message)}
    </p>
  ) : null;
}

function Field({
  label,
  name,
  type = "password",
  autoComplete,
  minLength,
}: {
  label: string;
  name: string;
  type?: string;
  autoComplete?: string;
  minLength?: number;
}) {
 const { t } = useI18n();
  return (
    <label className="field">
      {t(label)}
      <input
        name={name}
        type={type}
        autoComplete={autoComplete}
        minLength={minLength}
        required
      />
    </label>
  );
}

export function AuthPanel({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
 const { t } = useI18n();
  return (
    <section className="auth-panel">
      <h1>{t(title)}</h1>
      {children}
    </section>
  );
}

export function SignInForm({ restartURL }: { restartURL: string }) {
 const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    setBusy(true);
    setError("");
    try {
      const result = await authClient.signIn.email({
        email: String(form.get("email")),
        password: String(form.get("password")),
      });
      if (result.error)
        setError(
          signInError(result.error, "Sign-in failed. Check your email and password."),
        );
      else goToAccount(result.data);
    } catch {
      setError("We couldn’t connect. Try signing in again.");
    } finally {
      setBusy(false);
    }
  }
  return (
    <AuthPanel title={t("Sign in to Webdock")}>
      <p className="muted">{t("One account for your workspace and websites.")}</p>
      <form method="post" onSubmit={submit} aria-busy={busy}>
        <Field
          label={t("Email address")}
          name="email"
          type="email"
          autoComplete="username"
        />
        <Field
          label={t("Password")}
          name="password"
          autoComplete="current-password"
        />
        <Notice message={error} error />
        {error === SIGN_IN_EXPIRED && <a className="button secondary" href={restartURL}>{t("Start a new sign-in")}</a>}
        <button className="button" disabled={busy}>
          {busy ? t("Signing in…") : t("Sign in")}
        </button>
      </form>
      <button
        type="button"
        className="button secondary"
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          setError("");
          try {
            if (!window.PublicKeyCredential)
              throw new Error(
                "This browser does not support passkeys. Sign in with your password.",
              );
            const result = await authClient.signIn.passkey();
            if (result.error)
              setError(
                signInError(result.error, "Passkey sign-in was cancelled. Try again or use your password."),
              );
            else goToAccount(result.data);
          } catch (cause) {
            setError(
              cause instanceof Error
                ? cause.message
                : "Passkey sign-in failed. Try again or use your password.",
            );
          } finally {
            setBusy(false);
          }
        }}
      >
        {t("Sign in with a passkey")}</button>
      <p className="help">
        {t("You can add a passkey in your account after signing in.")}</p>
      <Link href="/forgot-password">{t("Forgot your password?")}</Link>
      <p className="help">
        {t("Access is by invitation. If you need access or help with your account, contact your Webdock administrator.")}</p>
    </AuthPanel>
  );
}

export function TwoFactorForm({ restartURL }: { restartURL: string }) {
 const { t } = useI18n();
  const [recovery, setRecovery] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const code = String(new FormData(event.currentTarget).get("code")).trim();
    setBusy(true);
    setError("");
    try {
      const result = recovery
        ? await authClient.twoFactor.verifyBackupCode({
            code,
            trustDevice: false,
          })
        : await authClient.twoFactor.verifyTotp({ code, trustDevice: false });
      if (result.error)
        setError(
          signInError(result.error, "The code could not be verified. Try again."),
        );
      else goToAccount(result.data);
    } catch {
      setError("We couldn’t connect. Try verifying your code again.");
    } finally {
      setBusy(false);
    }
  }
  return (
    <AuthPanel title={recovery ? t("Use a recovery code") : t("Verify your sign-in")}>
      <p className="muted">
        {recovery ? t("Enter one of the recovery codes you saved. Each code works once.") : t("Enter the six-digit code from your authenticator app.")}
      </p>
      <form method="post" onSubmit={submit} aria-busy={busy}>
        <label className="field">
          {recovery ? t("Recovery code") : t("Authenticator code")}
          <input
            key={String(recovery)}
            name="code"
            type="text"
            inputMode={recovery ? "text" : "numeric"}
            autoComplete="one-time-code"
            pattern={recovery ? undefined : "[0-9]{6}"}
            maxLength={recovery ? 100 : 6}
            required
          />
        </label>
        <Notice message={error} error />
        {error === SIGN_IN_EXPIRED && <a className="button secondary" href={restartURL}>{t("Start a new sign-in")}</a>}
        <button className="button" disabled={busy}>
          {busy ? t("Verifying…") : t("Verify")}
        </button>
      </form>
      <button
        className="text-button"
        disabled={busy}
        onClick={() => {
          setRecovery(!recovery);
          setError("");
        }}
      >
        {recovery ? t("Use an authenticator code") : t("Use a recovery code")}
      </button>
    </AuthPanel>
  );
}

export function Account({ links }: { links: { tenants: string; sites: string; people: string; admin: string; studio: string } }) {
 const { t } = useI18n();
  const {
    data: session,
    isPending,
    error: sessionError,
    refetch,
  } = accountClient.useSession();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [setup, setSetup] = useState<{
    totpURI: string;
    backupCodes: string[];
  } | null>(null);
  const [showCodes, setShowCodes] = useState(false);
  const [savedCodes, setSavedCodes] = useState(false);
  async function act(action: () => Promise<void>) {
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await action();
    } catch (cause) {
      setError(
        cause instanceof Error
          ? cause.message
          : "Something went wrong. Try again.",
      );
    } finally {
      setBusy(false);
    }
  }
  function check(error: { message?: string } | null) {
    if (error)
      throw new Error(error.message || "The request failed. Try again.");
  }
  if (isPending)
    return (
      <AuthPanel title={t("Your account")}>
        <p role="status">{t("Loading your account…")}</p>
      </AuthPanel>
    );
  if (sessionError)
    return (
      <AuthPanel title={t("Your account")}>
        <Notice
          message="We couldn’t load your account. Please reload this page."
          error
        />
        <button className="button" onClick={() => void refetch()}>
          {t("Try again")}</button>
      </AuthPanel>
    );
  if (!session)
    return (
      <AuthPanel title={t("Your account")}>
        <p>{t("Sign in to manage your password and account security.")}</p>
        <button
          className="button"
          onClick={() =>
            window.location.assign(`/sign-in${window.location.search}`)
          }
        >
          {t("Sign in")}</button>
      </AuthPanel>
    );
  const needsSetup =
    session.user.mustChangePassword ||
    (session.user.role === "operator" && !session.user.twoFactorEnabled);
  return (
    <div className="account">
      <header className="account-heading">
        <div>
          <h1>{t("Your account")}</h1>
          <p className="muted">
            {session.user.name}
            <br />
            {session.user.email}
          </p>
        </div>
        <button
          className="button secondary"
          disabled={busy}
          onClick={() =>
            void act(async () => {
              const result = await accountClient.signOut();
              check(result.error);
              window.location.assign("/sign-in");
            })
          }
        >
          {t("Sign out")}</button>
      </header>
      <nav className="actions" aria-label={t("Workspace shortcuts")}><a className="button secondary" href={links.tenants}>{t("My tenants")}</a><a className="button secondary" href={links.sites}>{t("My websites")}</a>{session.user.role === "operator" && <><a className="button secondary" href={links.people}>{t("People & access")}</a><a className="button secondary" href={links.admin}>{t("Webdock administration")}</a></>}</nav>
      {needsSetup && (<p className="notice">
          {t("Complete your account security setup before continuing to your workspace.")}</p>)}
      <Notice message={error} error />
      <Notice message={message} />
      <section className="account-section">
        <h2>{t("Change password")}</h2>
        <p className="muted">
          {session.user.mustChangePassword ? t("Replace your temporary password with a password only you know.") : t("Use a unique password with at least 12 characters.")}
        </p>
        <form
          method="post"
          onSubmit={(event) => {
            event.preventDefault();
            const form = event.currentTarget;
            const values = new FormData(form);
            void act(async () => {
              if (values.get("newPassword") !== values.get("confirmPassword"))
                throw new Error("The new passwords do not match.");
              const result = await accountClient.changePassword({
                currentPassword: String(values.get("currentPassword")),
                newPassword: String(values.get("newPassword")),
                revokeOtherSessions: true,
              });
              check(result.error);
              form.reset();
              await refetch();
              setMessage(
                "Password changed. Other sessions have been signed out.",
              );
            });
          }}
          aria-busy={busy}
        >
          <Field
            label={t("Current password")}
            name="currentPassword"
            autoComplete="current-password"
          />
          <Field
            label={t("New password")}
            name="newPassword"
            autoComplete="new-password"
            minLength={12}
          />
          <Field
            label={t("Confirm new password")}
            name="confirmPassword"
            autoComplete="new-password"
            minLength={12}
          />
          <button className="button" disabled={busy}>
            {t("Change password")}</button>
        </form>
      </section>
      <section className="account-section">
        <h2>{t("Two-factor authentication")}</h2>
        {session.user.twoFactorEnabled ? (<p className="notice">{t("Your authenticator is enabled.")}</p>) : (<>
            <p className="muted">
              {t("Protect your account with a code from an authenticator app. Required for Webdock operators.")}</p>
            {!setup ? (<form
                method="post"
                onSubmit={(event) => {
                  event.preventDefault();
                  const form = event.currentTarget;
                  const password = String(new FormData(form).get("password"));
                  void act(async () => {
                    const result = await accountClient.twoFactor.enable({
                      password,
                      method: "totp",
                    });
                    check(result.error);
                    if (result.data && "totpURI" in result.data) {
                      setSetup({
                        totpURI: result.data.totpURI,
                        backupCodes: result.data.backupCodes,
                      });
                      form.reset();
                    }
                  });
                }}
                aria-busy={busy}
              >
                <Field
                  label={t("Current password")}
                  name="password"
                  autoComplete="current-password"
                />
                <button className="button" disabled={busy}>
                  {t("Set up authenticator")}</button>
              </form>) : (<div className="setup">
                <h3>{t("Add Webdock to your authenticator")}</h3>
                <p>
                  {t("Choose “Enter a setup key” in your app. Use your email as the account name and select a time-based code.")}</p>
                <label className="field">
                  {t("Setup key")}<input
                    readOnly
                    value={
                      new URL(setup.totpURI).searchParams.get("secret") || ""
                    }
                    spellCheck={false}
                  />
                </label>
                <details>
                  <summary>{t("Full authenticator URI")}</summary>
                  <code>{setup.totpURI}</code>
                </details>
                <h3>{t("Save your recovery codes")}</h3>
                <p>
                  {t("Store these somewhere safe. You will need a recovery code if you lose your authenticator.")}</p>
                <button
                  className="button secondary"
                  type="button"
                  onClick={() => setShowCodes(!showCodes)}
                >
                  {showCodes ? t("Hide recovery codes") : t("Show recovery codes")}
                </button>
                {showCodes && (<ul className="recovery-codes">
                    {setup.backupCodes.map((code) => (
                      <li key={code}>
                        <code>{code}</code>
                      </li>
                    ))}
                  </ul>)}
                <form
                  method="post"
                  onSubmit={(event) => {
                    event.preventDefault();
                    const code = String(
                      new FormData(event.currentTarget).get("code"),
                    );
                    void act(async () => {
                      const result = await accountClient.twoFactor.verifyTotp({
                        code,
                        trustDevice: false,
                      });
                      check(result.error);
                      setSetup(null);
                      setShowCodes(false);
                      setSavedCodes(false);
                      await refetch();
                      setMessage("Your authenticator is ready.");
                    });
                  }}
                  aria-busy={busy}
                >
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={savedCodes}
                      onChange={(event) => setSavedCodes(event.target.checked)}
                      required
                    />{" "}
                    {t("I have saved my recovery codes.")}</label>
                  <label className="field">
                    {t("Authenticator code")}<input
                      name="code"
                      inputMode="numeric"
                      autoComplete="one-time-code"
                      pattern="[0-9]{6}"
                      maxLength={6}
                      required
                    />
                  </label>
                  <button className="button" disabled={busy || !savedCodes}>
                    {t("Verify and enable")}</button>
                </form>
              </div>)}
          </>)}
      </section>
      {needsSetup ? (<section className="account-section">
          <h2>{t("Passkeys")}</h2>
          <p className="muted">
            {t("Complete your password and authenticator setup before adding a passkey.")}</p>
        </section>) : (<Passkeys disabled={busy} />)}
      <section className="account-section">
        <h2>{t("Continue to your workspace")}</h2>
        <p className="muted">
          {t("Once your account is ready, continue the sign-in you started or return to Webdock.")}</p>
        <button
          className="button"
          disabled={busy || !!needsSetup}
          onClick={() =>
            void act(async () => {
              if (!new URLSearchParams(window.location.search).has("sig")) {
                window.location.assign(
                  session.user.role === "operator" ? links.studio : links.tenants,
                );
                return;
              }
              const result = await authClient.oauth2.continue({
                postLogin: true,
              });
              check(result.error);
            })
          }
        >
          {t("Continue")}</button>
      </section>
    </div>
  );
}

export function ForgotPasswordForm() {
 const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState(false);
  const [error, setError] = useState("");
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const email = String(new FormData(event.currentTarget).get("email"));
    setBusy(true);
    setError("");
    try {
      const result = await accountClient.requestPasswordReset({
        email,
        redirectTo: new URL("/reset-password", window.location.origin).href,
      });
      if (result.error)
        setError(
          "We couldn’t request a password reset. Please try again later.",
        );
      else setSent(true);
    } catch {
      setError("We couldn’t connect. Please try again.");
    } finally {
      setBusy(false);
    }
  }
  return (
    <AuthPanel title={t("Reset your password")}>
      {sent ? (<Notice message="If an account exists for that email address, you’ll receive a password reset link. Check your inbox and spam folder." />) : (<>
          <p className="muted">
            {t("Enter your account’s email address to request a reset link.")}</p>
          <form method="post" onSubmit={submit} aria-busy={busy}>
            <Field
              label={t("Email address")}
              name="email"
              type="email"
              autoComplete="username"
            />
            <Notice message={error} error />
            <button className="button" disabled={busy}>
              {busy ? t("Requesting link…") : t("Send reset link")}
            </button>
          </form>
        </>)}
      <Link href="/sign-in">{t("Back to sign in")}</Link>
    </AuthPanel>
  );
}

export function ResetPasswordForm({ token, invitation }: { token: string | null; invitation?: string }) {
 const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const [error, setError] = useState("");
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!token) return;
    const form = event.currentTarget;
    const values = new FormData(form);
    if (values.get("newPassword") !== values.get("confirmPassword")) {
      setError("The new passwords do not match.");
      return;
    }
    setBusy(true);
    setError("");
    try {
      const result = await accountClient.resetPassword({
        token,
        newPassword: String(values.get("newPassword")),
      });
      if (result.error)
        setError(
          "This password reset could not be completed. Check your password or request a new reset link.",
        );
      else {
        form.reset();
        setDone(true);
        window.history.replaceState(null, "", "/reset-password");
      }
    } catch {
      setError("We couldn’t connect. Please try again.");
    } finally {
      setBusy(false);
    }
  }
  return (
    <AuthPanel title={done ? t("Password updated") : t("Choose a new password")}>
      {done ? (<>
          <Notice message="Your password has been updated and your previous sessions have been signed out. Sign in with your new password." />
          <Link className="button" href={invitation ? `/sign-in?invitation=${encodeURIComponent(invitation)}` : "/sign-in"}>
            {t("Sign in")}</Link>
        </>) : !token ? (<>
          <Notice
            message="This reset link is invalid or has expired. Request a new link to reset your password."
            error
          />
          <Link className="button" href="/forgot-password">
            {t("Request a new link")}</Link>
        </>) : (<>
          <p className="muted">
            {t("Use a unique password with at least 12 characters.")}</p>
          <form method="post" onSubmit={submit} aria-busy={busy}>
            <Field
              label={t("New password")}
              name="newPassword"
              autoComplete="new-password"
              minLength={12}
            />
            <Field
              label={t("Confirm new password")}
              name="confirmPassword"
              autoComplete="new-password"
              minLength={12}
            />
            <Notice message={error} error />
            <button className="button" disabled={busy}>
              {busy ? t("Updating password…") : t("Update password")}
            </button>
          </form>
          <Link href="/forgot-password">{t("Request a new link")}</Link>
        </>)}
    </AuthPanel>
  );
}

export function ConsentForm({
  name,
  scopes,
  claims,
}: {
  name: string;
  scopes: string[];
  claims: string[];
}) {
 const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function decide(accept: boolean) {
    setBusy(true);
    setError("");
    try {
      const result = await authClient.oauth2.consent({ accept });
      if (result.error)
        setError(
          result.error.message ||
            "Could not complete authorization. Try again.",
        );
    } catch {
      setError("We couldn’t connect. Try again.");
    } finally {
      setBusy(false);
    }
  }
  return (
    <AuthPanel title={t("Authorize access")}>
      <p>
        <strong>{name}</strong> {t(" is requesting access to your account.")}</p>
      <h2>{t("Requested permissions")}</h2>
      <ul>
        {scopes.map((scope) => (
          <li key={scope}>{({"hosting:read": t("Read your hosting projects and resource usage"), "hosting:write": t("Manage hosting within your permissions and limits"), "webdock:read": t("Read customers, projects, CMS connections and activity"), "webdock:write": t("Create and update registry records"), offline_access: t("Automatically renew access during your working session"), openid: t("Confirm your identity"), profile: t("Basic profile"), email: t("Email address")} as Record<string, string>)[scope] || scope}</li>
        ))}
      </ul>
      {claims.length > 0 && (<>
          <h2>{t("Requested profile fields")}</h2>
          <ul>
            {claims.map((claim) => (
              <li key={({name: t("Name"), email: t("Email address"), email_verified: t("Email verified"), picture: t("Profile picture")} as Record<string,string>)[claim] || claim}>{({name: t("Name"), email: t("Email address"), email_verified: t("Email verified"), picture: t("Profile picture")} as Record<string,string>)[claim] || claim}</li>
            ))}
          </ul>
        </>)}
      <Notice message={error} error />
      <div className="actions">
        <button
          className="button"
          disabled={busy}
          onClick={() => void decide(true)}
        >
          {t("Allow access")}</button>
        <button
          className="button secondary"
          disabled={busy}
          onClick={() => void decide(false)}
        >
          {t("Deny")}</button>
      </div>
    </AuthPanel>
  );
}
