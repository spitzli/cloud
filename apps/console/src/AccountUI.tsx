import { DeveloperAccess } from "./DeveloperAccess";
import { tenantsSchema } from "./workspace-contracts";
import { WorkspaceAvatar } from "./WorkspaceAvatar";
import { AuthHeader, AccountBrand, PageFailure, PageProgress, accountFrame as authFrame, accountPanel as authPanel } from "./AuthFrame";
import { ThemeButton } from "./theme";
import { LocalePicker } from "./Locale";
import { formText } from "./form";
import { useEffect, useRef, useState, type ReactNode, type SubmitEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  startAuthentication,
  startRegistration,
  browserSupportsWebAuthn,
  type PublicKeyCredentialCreationOptionsJSON,
  type PublicKeyCredentialRequestOptionsJSON,
} from "@simplewebauthn/browser";
import { QRCodeSVG } from "qrcode.react";
import { PasswordIcon } from "@phosphor-icons/react/dist/csr/Password";
import { ShieldCheckIcon } from "@phosphor-icons/react/dist/csr/ShieldCheck";
import { BuildingsIcon } from "@phosphor-icons/react/dist/csr/Buildings";
import { FingerprintIcon } from "@phosphor-icons/react/dist/csr/Fingerprint";
import { UserMenu } from "./UserMenu";
import { FieldError, useLiveValidation } from "./live-validation";
import { CustomerFields } from "./CustomerFields";
import { KeyIcon } from "@phosphor-icons/react/dist/csr/Key";
import { DevicesIcon } from "@phosphor-icons/react/dist/csr/Devices";
import { z } from "zod";
import { useApi } from "./context";
import { type RegistryRecord, type Session } from "./api";
import { sessionCacheScope, useSessionCacheIsolation } from "./session-cache";
import { Failure, Loading, Modal } from "./common";
import * as ui from "./ui";
const panelForm = "mt-5 grid gap-5";
const buttonRow = "mt-5 flex flex-wrap gap-3";
const sectionHeading = `${ui.sectionTitle} flex items-center gap-2 [&>svg]:shrink-0 [&>svg]:text-muted`;
const disclosure = "group rounded-lg border border-outline [&>summary]:cursor-pointer [&>summary]:px-4 [&>summary]:py-2.5 [&>summary]:text-sm [&>summary]:font-medium";
import {
  authorizationContinuation,
  nativeSessionSchema,
  resultSchema,
  sessionListSchema,
  passkeyListSchema,
} from "./account-contracts";

function form(event: SubmitEvent<HTMLFormElement>) {
  event.preventDefault();
  return new FormData(event.currentTarget);
}
function PasswordField({
  name = "password",
  label = "Password",
  fresh = false,
}: {
  name?: string;
  label?: string;
  fresh?: boolean;
}) {
  const [touched, setTouched] = useState(false);
  const [message, setMessage] = useState("");
  const check = (input: HTMLInputElement) => {
    const form = input.form;
    if (name === "confirmPassword") {
      const next = form?.elements.namedItem("newPassword") ?? form?.elements.namedItem("password");
      input.setCustomValidity(next instanceof HTMLInputElement && next.value !== input.value ? "The passwords do not match." : "");
    }
    if (fresh && name !== "confirmPassword") {
      // Keep the confirmation's live result in step with the new password.
      const confirmation = form?.elements.namedItem("confirmPassword");
      if (confirmation instanceof HTMLInputElement && confirmation.value) confirmation.dispatchEvent(new Event("input", { bubbles: true }));
    }
    setMessage(input.validity.valid ? "" : input.validationMessage);
  };
  return (
    <>
    <label className={ui.field}>
      {label}
      <input
        className={ui.input}
        name={name}
        type="password"
        required
        minLength={fresh ? 12 : undefined}
        maxLength={128}
        autoComplete={fresh ? "new-password" : "current-password"}
        onBlur={(event) => { setTouched(true); check(event.currentTarget); }}
        onInput={(event) => { if (touched || name === "confirmPassword") check(event.currentTarget); }}
      />
    </label>
      {fresh && name !== "confirmPassword" && !(touched && message) ? <span className={ui.cx(`${ui.hint} -mt-3`)}>At least 12 characters.</span> : null}
      <FieldError message={touched ? message : undefined} />
    </>
  );
}
function Feedback({
  error,
  message,
}: {
  error?: Error | null;
  message?: string | undefined;
}) {
  return (
    <>
      {error ? (
        <p className="text-sm text-danger" role="alert">
          {error.message}
        </p>
      ) : null}
      {message ? (
        <p className="text-sm text-success" role="status">
          {message}
        </p>
      ) : null}
    </>
  );
}
function AccountSignIn({ onSuccess }: { onSuccess: () => void }) {
  const api = useApi();
  const [mode, setMode] = useState<"password" | "totp" | "recovery" | "reset">(
    "password",
  );
  const signIn = useMutation({
    mutationFn: async (data: FormData) => {
      const fields =
        mode === "password"
          ? { email: data.get("email"), password: data.get("password") }
          : mode === "reset"
            ? { email: data.get("email") }
            : { code: data.get("code") };
      const path =
        mode === "password"
          ? "sign-in/email"
          : mode === "reset"
            ? "request-password-reset"
            : mode === "totp"
              ? "two-factor/verify-totp"
              : "two-factor/verify-backup-code";
      return api.write(`/api/auth/${path}`, "POST", fields, resultSchema, true);
    },
    onSuccess: (result) => {
      if (mode === "reset") return;
      if (result.twoFactorRedirect) setMode("totp");
      else onSuccess();
    },
  });
  const passkey = useMutation({
    mutationFn: async () => {
      const options = await api.request(
        "/api/auth/passkey/generate-authenticate-options",
        z.custom<PublicKeyCredentialRequestOptionsJSON>(
          (value) =>
            typeof value === "object" && value !== null && "challenge" in value,
        ),
        {},
        true,
      );
      const response = await startAuthentication({ optionsJSON: options });
      return api.write(
        "/api/auth/passkey/verify-authentication",
        "POST",
        { response },
        resultSchema,
        true,
      );
    },
    onSuccess,
  });
  const choose = (next: typeof mode) => {
    setMode(next);
    signIn.reset();
    passkey.reset();
  };
  const busy = signIn.isPending || passkey.isPending;
  return (
    <main className={authFrame}>
      <AuthHeader product="Account" href="/account" />
      <section className={authPanel}>
        <h1>
          {mode === "password"
            ? "Sign in to Spitzli Cloud"
            : mode === "reset"
              ? "Reset your password"
              : "Verify your identity"}
        </h1>
        <p>
          {mode === "password"
            ? "One account for your workspaces and services. First time signing in? Use Forgot password to set your password."
            : mode === "reset"
              ? "Enter your account email to request a reset link."
              : mode === "totp"
                ? "Enter the six-digit code from your authenticator."
                : "Enter one of your unused recovery codes."}
        </p>
        {/* A new form per step: password managers must not reuse the email field for the code. */}
        <form
          key={mode}
          className={panelForm}
          onSubmit={(event) => {
            signIn.mutate(form(event));
          }}
        >
          <fieldset disabled={busy} className="contents">
          {mode === "password" || mode === "reset" ? (
            <label className={ui.field}>
              Email
              <input
                className={ui.input}
                name="email"
                type="email"
                required
                autoComplete="username"
                maxLength={254}
                autoFocus
              />
            </label>
          ) : (
            <label className={ui.field}>
              {mode === "totp" ? "Authentication code" : "Recovery code"}
              <input
                className={ui.input}
                name="code"
                required
                inputMode={mode === "totp" ? "numeric" : "text"}
                pattern={mode === "totp" ? "[0-9]{6}" : undefined}
                maxLength={64}
                autoComplete="one-time-code"
                autoFocus
              />
            </label>
          )}
          {mode === "password" ? <PasswordField /> : null}
          <Feedback
            error={signIn.error ?? passkey.error}
            message={
              mode === "reset" && signIn.isSuccess
                ? "If this account is eligible, recovery instructions will be sent."
                : undefined
            }
          />
          <button
            className={ui.cx(`${ui.button("primary")} w-full`)}
            disabled={busy}
          >
            {signIn.isPending
              ? "Checking…"
              : mode === "password"
                ? "Sign in"
                : mode === "reset"
                  ? "Send reset link"
                  : "Verify code"}
          </button>
          </fieldset>
        </form>
        {mode === "password" ? (
          <>
            <button
              className={ui.cx(`${ui.button()} mt-3.5 w-full`)}
              onClick={() => {
                passkey.mutate();
              }}
              disabled={busy || !browserSupportsWebAuthn()}
            >
              {passkey.isPending
                ? "Waiting for your passkey…"
                : "Sign in with a passkey"}
            </button>
            <button
              className={ui.textButton}
              disabled={busy}
              onClick={() => {
                choose("reset");
              }}
            >
              Forgot password?
            </button>
          </>
        ) : mode === "reset" ? (
          <button
            className={ui.textButton}
            disabled={busy}
            onClick={() => {
              choose("password");
            }}
          >
            Back to sign in
          </button>
        ) : (
          <div className="flex flex-col">
          <button className={ui.textButton} disabled={busy || !browserSupportsWebAuthn()} onClick={() => { passkey.mutate(); }}>
            {passkey.isPending ? "Waiting for your passkey…" : "Verify with a passkey"}
          </button>
          <button
            className={ui.textButton}
            disabled={busy}
            onClick={() => {
              choose(mode === "totp" ? "recovery" : "totp");
            }}
          >
            {mode === "totp"
              ? "Use a recovery code"
              : "Use an authenticator code"}
          </button>
          <button className={ui.textButton} disabled={busy} onClick={() => { choose("password"); }}>Back to sign in</button>
          </div>
        )}
      </section>
    </main>
  );
}
function PasswordReset() {
  const api = useApi();
  const query = new URLSearchParams(location.search);
  const token = query.get("token");
  // An invitation's activation link: create the account, sign in, then continue to the invitation.
  const invitation = query.get("invitation");
  const reset = useMutation({
    mutationFn: async (data: FormData) => {
      if (data.get("password") !== data.get("confirmPassword"))
        throw new Error("The passwords do not match.");
      const result = await api.write(
        "/api/auth/reset-password",
        "POST",
        { token, newPassword: data.get("password") },
        z.object({ email: z.string().optional() }),
        true,
      );
      if (!invitation) return;
      if(!result.email){location.assign(`/login?${new URLSearchParams({returnTo:`/invitation?id=${encodeURIComponent(invitation)}`})}`);return;}
      const signIn = await api.write(
        "/api/auth/sign-in/email",
        "POST",
        { email: result.email, password: data.get("password") },
        resultSchema,
        true,
      );
      location.assign(signIn.twoFactorRedirect ? "/login" : `/invitation?${new URLSearchParams({ id: invitation, welcome: "1" })}`);
    },
  });
  return (
    <main className={authFrame}>
      <AuthHeader product="Account" href="/account" />
      <section className={authPanel}>
        <h1>{invitation ? "Create your account" : "Choose a new password"}</h1>
        {invitation ? <p>Choose a password for your new account. You can add a passkey next.</p> : null}
        {!token ? (
          <p role="alert">
            This link is incomplete.{" "}
            <a href="/login">Request another reset link.</a>
          </p>
        ) : reset.isSuccess && !invitation ? (
          <p role="status">
            Your password is ready.{" "}
            <a className="underline underline-offset-[3px]" href="/login">
              Sign in
            </a>
          </p>
        ) : (
          <form
            className={panelForm}
            onSubmit={(event) => {
              reset.mutate(form(event));
            }}
          >
            <PasswordField label={invitation ? "Password" : "New password"} fresh />
            <PasswordField
              name="confirmPassword"
              label={invitation ? "Confirm password" : "Confirm new password"}
              fresh
            />
            <p className={ui.hint}>
              {invitation ? "Use at least 12 characters." : "Use at least 12 characters. Previous sessions will be signed out."}
            </p>
            <Feedback error={reset.error} />
            <button className={ui.cx(`${ui.button("primary")} w-full`)} disabled={reset.isPending || reset.isSuccess}>
              {reset.isPending || reset.isSuccess ? (invitation ? "Creating…" : "Saving…") : invitation ? "Create account" : "Save password"}
            </button>
          </form>
        )}
      </section>
    </main>
  );
}
function RecoveryCodes({ codes, account, done }: { codes: string[]; account: string; done: () => void }) {
  // Confirmation stays unavailable until the codes have left this page at least once.
  const [kept, setKept] = useState(false);
  const [status, setStatus] = useState("");
  const text = `Spitzli Cloud recovery codes for ${account}\nCreated ${new Date().toISOString().slice(0, 10)}. Each code works once.\n\n${codes.join("\n")}\n`;
  const copy = () => {
    void navigator.clipboard.writeText(text).then(() => {
      setKept(true);
      setStatus("Copied to your clipboard.");
    }, () => {
      setStatus("Copying is unavailable in this browser. Download the codes instead.");
    });
  };
  const download = () => {
    const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = "spitzli-cloud-recovery-codes.txt";
    anchor.click();
    setTimeout(() => { URL.revokeObjectURL(url); }, 1000);
    setKept(true);
    setStatus("Downloaded spitzli-cloud-recovery-codes.txt.");
  };
  return (
    <Modal title="Save your recovery codes" close={() => undefined} dismissible={false}>
      <div className="flex flex-col gap-4">
        <p className="text-muted">
          Keep these somewhere private, such as your password manager. Each code
          signs you in once if you lose your authenticator.
        </p>
        <ul className="recovery-codes grid max-w-[460px] list-none grid-cols-2 gap-x-6 gap-y-2.5 rounded-lg bg-soft p-4 font-mono text-[13px] [&_code]:select-all max-sm:gap-x-3 max-sm:text-xs" aria-label="Recovery codes">
          {codes.map((code) => (
            <li key={code}>
              <code>{code}</code>
            </li>
          ))}
        </ul>
        <div className="flex flex-wrap gap-2">
          <button className={ui.button()} onClick={copy}>Copy codes</button>
          <button className={ui.button()} onClick={download}>Download codes</button>
        </div>
        <p role="status" className="min-h-5 text-[13px] text-muted">{status || "Copy or download the codes to continue."}</p>
        <button className={ui.cx(`${ui.button("primary")} self-start`)} disabled={!kept} onClick={done}>
          I saved my recovery codes
        </button>
      </div>
    </Modal>
  );
}
function accountCanContinue(session: Session | null | undefined): boolean {
  return Boolean(
    session &&
    !session.user.preview &&
    session.user.emailVerified &&
    !session.user.mustChangePassword &&
    (session.user.role !== "operator" || session.user.twoFactorEnabled),
  );
}

function accountPrincipal(session: Session | null | undefined) {
  return session && !session.user.preview
    ? JSON.stringify([session.user.id, session.session.userId])
    : null;
}
function accountSecretScope(session: Session | null | undefined) {
  const principal = accountPrincipal(session);
  return session &&
    principal &&
    session.user.emailVerified &&
    !session.user.mustChangePassword
    ? JSON.stringify([principal, session.session.id, session.user.role ?? null])
    : null;
}
function confirmedMfaSession(
  before: Session,
  after: Session | null,
): after is Session {
  return Boolean(
    after &&
    accountCanContinue(after) &&
    after.user.twoFactorEnabled &&
    before.user.id === after.user.id &&
    before.session.userId === after.session.userId &&
    before.user.role === after.user.role &&
    !before.user.preview,
  );
}
function useAccountFeedback(session: Session | null | undefined) {
  const principal = accountPrincipal(session);
  const secretScope = accountSecretScope(session);
  const mfaEnabled = session?.user.twoFactorEnabled === true;
  const [stored, setStored] = useState({
    principal,
    secretScope,
    mfaEnabled,
    owner: {},
    passwordUpdated: false,
    confirmationError: null as string | null,
    codes: null as string[] | null,
  });
  let current = stored;
  if (
    stored.principal !== principal ||
    stored.secretScope !== secretScope ||
    (stored.mfaEnabled && !mfaEnabled)
  ) {
    current = {
      principal,
      secretScope,
      mfaEnabled,
      owner: {},
      passwordUpdated:
        principal !== null &&
        stored.principal === principal &&
        stored.passwordUpdated,
      confirmationError:
        principal !== null && stored.principal === principal
          ? stored.confirmationError
          : null,
      codes: null,
    };
  }
  if (current.mfaEnabled !== mfaEnabled) current = { ...current, mfaEnabled };
  if (current !== stored) setStored(current);
  const owner = current.owner;
  return {
    passwordUpdated: current.passwordUpdated,
    confirmationError: current.confirmationError,
    codes: current.codes,
    adoptConfirmedCodes: (confirmed: Session, codes: string[]) => {
      setStored((previous) =>
        previous.owner === owner &&
        previous.principal === accountPrincipal(confirmed)
          ? {
              ...previous,
              secretScope: accountSecretScope(confirmed),
              mfaEnabled: true,
              codes,
              confirmationError: null,
              owner: {},
            }
          : previous,
      );
    },
    setConfirmationError: (confirmationError: string | null) => {
      setStored((previous) =>
        previous.owner === owner && previous.principal !== null
          ? { ...previous, confirmationError }
          : previous,
      );
    },
    setPasswordUpdated: (passwordUpdated: boolean) => {
      setStored((previous) =>
        previous.owner === owner && previous.principal !== null
          ? { ...previous, passwordUpdated }
          : previous,
      );
    },
    setCodes: (codes: string[] | null) => {
      setStored((previous) =>
        previous.owner === owner && previous.secretScope !== null
          ? { ...previous, codes, confirmationError: null }
          : previous,
      );
    },
  };
}

function canFinishOwnedSetup(
  session: Session | null | undefined,
  feedback: ReturnType<typeof useAccountFeedback>,
) {
  return (
    feedback.codes !== null ||
    (feedback.passwordUpdated && !accountCanContinue(session))
  );
}

export function AccountSecurity({
  session,
  refresh,
  feedback,
  confirmEnrollment,
}: {
  session: Session;
  refresh: () => void;
  feedback: ReturnType<typeof useAccountFeedback>;
  confirmEnrollment: (code: string, codes: string[]) => Promise<void>;
}) {
  const api = useApi();
  const client = useQueryClient();
  const [enrollment, setEnrollment] = useState<{
    totpURI: string;
    backupCodes: string[];
  } | null>(null);
  const { codes, setCodes } = feedback;
  const [enrolled, setEnrolled] = useState(false);
  const [passkeyName, setPasskeyName] = useState("");
  const ready = !session.user.mustChangePassword && session.user.emailVerified;
  // Say why adding a passkey is unavailable instead of only disabling the button.
  const passkeyBlocked = session.user.mustChangePassword
    ? "Set your own password before adding a passkey."
    : !session.user.emailVerified
      ? "Verify your email address before adding a passkey."
      : !browserSupportsWebAuthn()
        ? "This browser does not support passkeys."
        : session.user.role === "operator" && !session.user.twoFactorEnabled
          ? "Operators set up an authenticator app first, then can add passkeys."
          : undefined;
  const change = useMutation({
    mutationFn: async (data: FormData) => {
      if (data.get("newPassword") !== data.get("confirmPassword"))
        throw new Error("The passwords do not match.");
      return api.write(
        "/api/auth/change-password",
        "POST",
        {
          currentPassword: data.get("currentPassword"),
          newPassword: data.get("newPassword"),
          revokeOtherSessions: true,
        },
        resultSchema,
        true,
      );
    },
    onMutate: () => {
      feedback.setPasswordUpdated(false);
    },
    onSuccess: () => {
      feedback.setPasswordUpdated(true);
      refresh();
    },
  });
  const enable = useMutation({
    gcTime: 0,
    mutationFn: (password: string) =>
      api.write(
        "/api/auth/two-factor/enable",
        "POST",
        { password },
        z.object({ totpURI: z.string(), backupCodes: z.array(z.string()) }),
        true,
      ),
    onSuccess: (value) => {
      setEnrollment(value);
      setEnrolled(false);
    },
  });
  const confirm = useMutation({
    mutationFn: (code: string) =>
      confirmEnrollment(code, enrollment?.backupCodes ?? []),
    onSuccess: () => {
      setEnrolled(true);
      setEnrollment(null);
    },
  });
  const recover = useMutation({
    gcTime: 0,
    mutationFn: (password: string) =>
      api.write(
        "/api/auth/two-factor/generate-backup-codes",
        "POST",
        { password },
        z.object({ backupCodes: z.array(z.string()) }),
        true,
      ),
    onSuccess: (value) => {
      setCodes(value.backupCodes);
    },
  });
  const disable = useMutation({
    mutationFn: (password: string) =>
      api.write(
        "/api/auth/two-factor/disable",
        "POST",
        { password },
        resultSchema,
        true,
      ),
    onSuccess: refresh,
  });
  const passkeys = useQuery({
    queryKey: ["account-passkeys"],
    queryFn: () =>
      api.request(
        "/api/auth/passkey/list-user-passkeys",
        passkeyListSchema,
        {},
        true,
      ),
    enabled:
      ready &&
      (session.user.role !== "operator" ||
        session.user.twoFactorEnabled === true),
  });
  const sessions = useQuery({
    queryKey: ["account-sessions"],
    queryFn: () =>
      api.request("/api/auth/list-sessions", sessionListSchema, {}, true),
  });
  const addPasskey = useMutation({
    mutationFn: async () => {
      const options = await api.request(
        "/api/auth/passkey/generate-register-options",
        z.custom<PublicKeyCredentialCreationOptionsJSON>(
          (value) =>
            typeof value === "object" && value !== null && "challenge" in value,
        ),
        {},
        true,
      );
      const response = await startRegistration({ optionsJSON: options });
      return api.write(
        "/api/auth/passkey/verify-registration",
        "POST",
        { response, name: passkeyName || "My passkey" },
        z.object({ id: z.string() }),
        true,
      );
    },
    onSuccess: () => {
      setPasskeyName("");
      void client.invalidateQueries({ queryKey: ["account-passkeys"] });
    },
  });
  const removePasskey = useMutation({
    mutationFn: (id: string) =>
      api.write(
        "/api/auth/passkey/delete-passkey",
        "POST",
        { id },
        resultSchema,
        true,
      ),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["account-passkeys"] });
    },
  });
  const renamePasskey = useMutation({
    mutationFn: ({ id, name }: { id: string; name: string }) =>
      api.write(
        "/api/auth/passkey/update-passkey",
        "POST",
        { id, name },
        resultSchema,
        true,
      ),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["account-passkeys"] });
    },
  });
  const revoke = useMutation({
    mutationFn: (id: string | null) =>
      api.write(
        id
          ? "/api/auth/revoke-session-by-id"
          : "/api/auth/revoke-other-sessions",
        "POST",
        id ? { id } : {},
        resultSchema,
        true,
      ),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["account-sessions"] });
    },
  });
  const completeCodes = () => {
    setCodes(null);
    enable.reset();
    recover.reset();
    confirm.reset();
    refresh();
  };
  const secret = enrollment
    ? new URL(enrollment.totpURI).searchParams.get("secret")
    : null;
  return (
    <div className="grid items-start gap-6 lg:grid-cols-2">
      <section className={ui.cx(`security-password ${ui.card} col-auto row-auto lg:row-span-2`)}>
        <form
          onSubmit={(event) => {
            const data = form(event);
            const target = event.currentTarget;
            change.mutate(data, {
              onSuccess: () => {
                target.reset();
              },
            });
          }}
        >
          <div className={ui.cx(`${ui.cardBody} gap-5`)}>
            <h2 className={sectionHeading}>
              <PasswordIcon size={20} aria-hidden />
              Change password
            </h2>
            {session.user.mustChangePassword ? (
              <p className={ui.notice} role="status">
                Set your own password to continue.
              </p>
            ) : null}
            <PasswordField name="currentPassword" label="Current password" />
            <PasswordField name="newPassword" label="New password" fresh />
            <PasswordField
              name="confirmPassword"
              label="Confirm new password"
              fresh
            />
            <Feedback
              error={change.error}
              message={feedback.passwordUpdated ? "Password updated." : undefined}
            />
          </div>
          <div className={ui.cardFooter}>
            <span>Other sessions will be signed out.</span>
            <button className={ui.button()} disabled={change.isPending}>
              {change.isPending ? "Saving…" : "Update password"}
            </button>
          </div>
        </form>
      </section>
      <section className={ui.card}>
        <div className={ui.cardBody}>
          <h2 className={sectionHeading}>
            <ShieldCheckIcon size={20} aria-hidden />
            Two-factor authentication
          </h2>
          {session.user.role === "operator" && !session.user.twoFactorEnabled ? (
            <p className={ui.notice}>
              Protect your operator account with an authenticator before
              continuing.
            </p>
          ) : null}
          {feedback.confirmationError ? (
            <p className="text-sm text-danger" role="alert">
              {feedback.confirmationError}
            </p>
          ) : null}
          {codes ? (
            <RecoveryCodes codes={codes} account={session.user.email} done={completeCodes} />
          ) : enrollment ? (
            <Modal title="Set up authenticator" close={() => { setEnrollment(null); confirm.reset(); enable.reset(); }}>
            <form
              className="grid gap-5"
              onSubmit={(event) => {
                confirm.mutate(formText(form(event), "code"));
              }}
            >
              <p>
                Scan this code with your authenticator, then enter its six-digit
                code.
              </p>
              <div className="w-fit rounded-lg bg-white p-2">
                <QRCodeSVG
                  value={enrollment.totpURI}
                  size={196}
                  marginSize={3}
                  title="Authenticator setup code"
                />
              </div>
              <details className={disclosure}>
                <summary>Enter a setup key instead</summary>
                <code className="setup-key block px-4 pb-3 font-mono text-[13px] [overflow-wrap:anywhere] select-all">{secret}</code>
              </details>
              <label className={ui.field}>
                Authentication code
                <input
                  className={ui.input}
                  name="code"
                  inputMode="numeric"
                  pattern="[0-9]{6}"
                  autoComplete="one-time-code"
                  required
                />
              </label>
              <Feedback error={confirm.error} />
              <div>
                <button className={ui.button("primary")} disabled={confirm.isPending}>
                  Verify and enable
                </button>
              </div>
            </form>
            </Modal>
          ) : session.user.twoFactorEnabled || enrolled ? (
            <>
              <p className="flex items-center gap-2 text-sm"><span className={ui.badge("success")}>Enabled</span>Your authenticator protects this account.</p>
              <details className={disclosure}>
                <summary>Replace recovery codes</summary>
                <form
                  className="grid gap-4 px-4 pb-4"
                  onSubmit={(event) => {
                    recover.mutate(formText(form(event), "password"));
                  }}
                >
                  <p className="text-sm text-muted">New codes replace all existing recovery codes.</p>
                  <PasswordField />
                  <Feedback error={recover.error} />
                  <div>
                    <button className={ui.button()} disabled={recover.isPending}>
                      Generate new recovery codes
                    </button>
                  </div>
                </form>
              </details>
              <details className={disclosure}>
                <summary>Turn off two-factor authentication</summary>
                <form
                  className="grid gap-4 px-4 pb-4"
                  onSubmit={(event) => {
                    disable.mutate(formText(form(event), "password"));
                  }}
                >
                  <p className="text-sm text-muted">
                    {session.user.role === "operator"
                      ? "Operator access pauses until you set up an authenticator again."
                      : "Your password will remain required to sign in."}
                  </p>
                  <PasswordField />
                  <Feedback error={disable.error} />
                  <div>
                    <button className={ui.button()} disabled={disable.isPending}>
                      Turn off two-factor authentication
                    </button>
                  </div>
                </form>
              </details>
            </>
          ) : (
            <form
              className="grid gap-4"
              onSubmit={(event) => {
                enable.mutate(formText(form(event), "password"));
              }}
            >
              <p className="text-sm text-muted">Add an authenticator for a second sign-in check.</p>
              <PasswordField />
              <Feedback error={enable.error} />
              <div>
                <button
                  className={ui.button("primary")}
                  disabled={!ready || enable.isPending}
                >
                  Set up authenticator
                </button>
              </div>
            </form>
          )}
        </div>
      </section>
      <section className={ui.cx(`security-passkeys ${ui.card} col-auto row-auto`)}>
        <div className={ui.cardBody}>
          <h2 className={sectionHeading}>
            <KeyIcon size={20} aria-hidden />
            Passkeys
          </h2>
          <p className="-mt-2 text-sm text-muted">Use a device screen lock or security key to sign in.</p>
          {passkeys.error ? <Feedback error={passkeys.error} /> : null}
        </div>
        <ul className="security-list">
          {passkeys.data?.map((passkey) => (
            <li key={passkey.id} className={ui.cx(`${ui.cardRow} border-b-0`)}>
              <form
                className="flex min-w-0 flex-1 items-center gap-2"
                onSubmit={(event) => {
                  renamePasskey.mutate({
                    id: passkey.id,
                    name: formText(form(event), "name"),
                  });
                }}
              >
                <label
                  className="sr-only"
                  htmlFor={`passkey-${passkey.id}`}
                >
                  Passkey name
                </label>
                <input
                  className={ui.cx(`${ui.input} min-h-9 max-w-64`)}
                  id={`passkey-${passkey.id}`}
                  name="name"
                  defaultValue={passkey.name ?? "Passkey"}
                  maxLength={100}
                  required
                />
                <button
                  className={ui.button("quiet")}
                  disabled={renamePasskey.isPending}
                >
                  Rename
                </button>
              </form>
              <button
                className={ui.button("quiet")}
                onClick={() => {
                  if (window.confirm("Remove this passkey?"))
                    removePasskey.mutate(passkey.id);
                }}
                disabled={removePasskey.isPending}
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
        {passkeys.data?.length === 0 ? (
          <p className={ui.cx(`${ui.cardRow} text-muted`)}>No passkeys yet.</p>
        ) : null}
        <form
          className={ui.cx(`${ui.cardFooter} items-end`)}
          onSubmit={(event) => {
            event.preventDefault();
            addPasskey.mutate();
          }}
        >
          <label className={ui.cx(`${ui.field} min-w-0 flex-1 text-foreground`)}>
            New passkey name
            <input
              className={ui.cx(`${ui.input} min-h-9`)}
              value={passkeyName}
              onChange={(event) => {
                setPasskeyName(event.target.value);
              }}
              placeholder="For example, work laptop"
              maxLength={100}
            />
          </label>
          <button
            className={ui.button()}
            disabled={Boolean(passkeyBlocked) || addPasskey.isPending}
          >
            {addPasskey.isPending
              ? "Waiting for your device…"
              : "Add a passkey"}
          </button>
          {passkeyBlocked ? <p className="w-full text-[13px] text-muted">{passkeyBlocked}</p> : null}
          <Feedback
            error={addPasskey.error ?? removePasskey.error ?? renamePasskey.error}
          />
        </form>
      </section>
      <section className={ui.cx(`security-section ${ui.card} gap-0 p-0 lg:col-span-2`)}>
        <div className={ui.cardBody}>
          <h2 className={sectionHeading}>
            <DevicesIcon size={20} aria-hidden />
            Sessions
          </h2>
          <p className="-mt-2 text-sm text-muted">Sign out sessions you no longer use.</p>
          {sessions.error ? <Feedback error={sessions.error} /> : null}
        </div>
        <ul>
          {sessions.data?.map((item) => (
            <li key={item.id} className={ui.cardRow}>
              <div>
                <strong className="flex items-center gap-2 font-medium">
                  {item.current ? "This session" : "Other session"}
                </strong>
                <p className="text-[13px] text-muted">
                  Started {new Date(item.createdAt).toLocaleString()}
                  <br />
                  Expires {new Date(item.expiresAt).toLocaleString()}
                </p>
              </div>
              {!item.current ? (
                <button
                  className={ui.button()}
                  disabled={revoke.isPending}
                  onClick={() => {
                    revoke.mutate(item.id);
                  }}
                >
                  Sign out
                </button>
              ) : null}
            </li>
          ))}
        </ul>
        <div className={ui.cardFooter}>
          <Feedback error={revoke.error} />
          <button
            className={ui.cx(`${ui.button()} ml-auto`)}
            disabled={revoke.isPending}
            onClick={() => {
              revoke.mutate(null);
            }}
          >
            Sign out other sessions
          </button>
        </div>
      </section>
    </div>
  );
}
function ConsentPage() {
  const api = useApi();
  const request = new URLSearchParams(location.search).get("request");
  const consent = useQuery({
    queryKey: ["consent", request],
    queryFn: () =>
      api.request(
        `/api/auth/oauth2/consent?request_id=${encodeURIComponent(request ?? "")}`,
        z.object({
          request_id: z.string(),
          client_name: z.string().nullable(),
          scopes: z.array(z.string()),
          resources: z.array(z.string()),
        }),
        {},
        true,
      ),
  });
  const answer = useMutation({
    mutationFn: (approved: boolean) =>
      api.write(
        "/api/auth/oauth2/consent",
        "POST",
        { request_id: request, approved },
        z.object({ redirect: z.url() }),
        true,
      ),
    onSuccess: (value) => {
      location.assign(value.redirect);
    },
  });
  if (consent.isPending) return <Loading>Loading request…</Loading>;
  if (consent.error)
    return (
      <Failure
        error={consent.error}
        retry={() => {
          void consent.refetch();
        }}
      />
    );
  const labels: Record<string, string> = {
    openid: "Confirm your identity",
    profile: "Read your name and profile",
    email: "Read your email address",
    offline_access: "Keep access while you are away",
  };
  return (
    <section className={authPanel}>
      <h1>
        Connect{" "}
        {consent.data.client_name?.trim()
          ? consent.data.client_name
          : "this application"}
        ?
      </h1>
      <p>This application is requesting access to your account.</p>
      <ul className="my-4 list-disc space-y-1.5 pl-5 text-sm">
        {consent.data.scopes.map((scope) => (
          <li key={scope}>{labels[scope] ?? scope}</li>
        ))}
      </ul>
      {consent.data.resources.length ? (
        <p>Services: {consent.data.resources.join(", ")}</p>
      ) : null}
      <Feedback error={answer.error} />
      <div className={buttonRow}>
        <button
          className={ui.button()}
          onClick={() => {
            answer.mutate(false);
          }}
          disabled={answer.isPending}
        >
          Cancel
        </button>
        <button
          className={ui.button("primary")}
          onClick={() => {
            answer.mutate(true);
          }}
          disabled={answer.isPending}
        >
          Allow access
        </button>
      </div>
    </section>
  );
}
function LogoutPage({ session }: { session: Session }) {
  const api = useApi();
  const request = new URLSearchParams(location.search).get("request");
  const details = useQuery({
    queryKey: ["logout", request],
    queryFn: () =>
      api.request(
        `/api/auth/browser/logout?request=${encodeURIComponent(request ?? "")}`,
        z.object({ applicationOrigin: z.url() }),
        {},
        true,
      ),
    enabled: !!request,
  });
  const answer = useMutation({
    mutationFn: async (confirmed: boolean) =>
      request
        ? api.write(
            "/api/auth/browser/logout",
            "POST",
            { request, confirmed },
            z.object({ redirect: z.url() }),
            true,
          )
        : confirmed
          ? api
              .write("/api/auth/sign-out", "POST", {}, resultSchema, true)
              .then(() => ({ redirect: "/login" }))
          : { redirect: "/account" },
    onSuccess: (value) => {
      location.assign(value.redirect);
    },
  });
  return (
    <section className={authPanel}>
      <h1>Sign out of Spitzli Cloud?</h1>
      <p>You are signed in as {session.user.email}.</p>
      <p>
        This ends the current account session and connected application access.
      </p>
      <Feedback error={details.error ?? answer.error} />
      <div className={buttonRow}>
        <button
          className={ui.button()}
          onClick={() => {
            answer.mutate(false);
          }}
          disabled={answer.isPending}
        >
          Stay signed in
        </button>
        <button
          className={ui.button("primary")}
          onClick={() => {
            answer.mutate(true);
          }}
          disabled={answer.isPending || !!details.error}
        >
          Sign out
        </button>
      </div>
    </section>
  );
}
/** Opens Console at a path through single sign-on; undefined when this installation has no Console. */
function useConsoleHref() {
  const api = useApi();
  return (path: string) => api.config.consoleOrigin ? `${api.config.consoleOrigin}/api/sso/login?returnTo=${encodeURIComponent(path)}` : undefined;
}
const roleText: Record<string, string> = {
  owner: "Owners manage the workspace, including its members, roles and services.",
  admin: "Administrators manage projects, members and customer details.",
  member: "Members work in the projects they are given access to.",
};
/** The organizations this account belongs to; their projects, apps, databases and sites live in Console. */
function WorkspacesPage() {
  const api = useApi();
  const consoleHref = useConsoleHref();
  const tenants = useQuery({
    queryKey: ["account-tenants"],
    queryFn: () => api.request("/api/tenants", tenantsSchema, {}, true),
  });
  const invitations = useQuery({
    queryKey: ["account-invitations"],
    queryFn: () => api.request("/api/invitations", z.array(z.object({ id: z.string(), role: z.string(), name: z.string() })), {}, true),
  });
  if (tenants.isPending || invitations.isPending) return <Loading>Loading your workspaces…</Loading>;
  const retry = () => { void tenants.refetch(); void invitations.refetch(); };
  if (tenants.error) return <Failure error={tenants.error} retry={retry} />;
  if (invitations.error) return <Failure error={invitations.error} retry={retry} />;
  return (
    <section>
      <div className={ui.pageHeading}>
        <div>
          <h1 className={ui.pageTitle}>Workspaces</h1>
          <p className={ui.pageLead}>Open a workspace in Console to manage its projects and services.</p>
        </div>
      </div>
      {invitations.data.length ? (
        <section aria-label="Invitations" className={ui.cx(ui.card, "mb-6 border-accent/40")}>
          <h2 className={ui.cx(ui.sectionTitle, "px-6 pt-5 pb-3 max-sm:px-[18px]")}>Waiting for you</h2>
          <ul>
            {invitations.data.map((invitation) => (
              <li key={invitation.id} className={ui.cx(ui.cardRow, ui.clickableRow)}>
                <span className="flex min-w-0 items-center gap-3">
                  <WorkspaceAvatar name={invitation.name} />
                  <span className="grid min-w-0"><strong className="truncate font-medium">{invitation.name}</strong><span className="text-[13px] text-muted">Invited as <span className="capitalize">{invitation.role}</span></span></span>
                </span>
                <a className={ui.cx(ui.button("primary"), ui.rowLink)} href={`/invitation?id=${encodeURIComponent(invitation.id)}`}>Review invitation</a>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      {tenants.data.length ? (
        <ul className={ui.card} aria-label="Your workspaces">
          {tenants.data.map((tenant) => {
            const href = consoleHref(`/workspaces/${tenant.id}`);
            return (
              <li key={tenant.id} className={ui.cx(ui.cardRow, "first:border-t-0", href && ui.clickableRow)}>
                <span className="flex min-w-0 items-center gap-3">
                  <WorkspaceAvatar name={tenant.name} icon={tenant.icon} />
                  <span className="grid min-w-0"><strong className="truncate font-medium">{tenant.name}</strong><span className="text-[13px] text-muted capitalize">{tenant.role ?? "Member"}</span></span>
                </span>
                <span className="flex items-center gap-2">
                  {tenant.status !== "active" ? <span className={ui.badge(ui.statusTone(tenant.status))}>{tenant.status}</span> : null}
                  {href ? <a className={ui.cx(ui.button(), ui.rowLink)} href={href}>Open in Console</a> : null}
                </span>
              </li>
            );
          })}
        </ul>
      ) : (
        <div className={ui.cx(ui.card, "px-7 py-10 max-sm:px-[18px] max-sm:py-6")}>
          <h2 className={ui.sectionTitle}>No workspaces yet</h2>
          <p className="mt-1.5 text-muted">When someone invites you to their organization, it appears here.</p>
        </div>
      )}
    </section>
  );
}
const profileKeys = ["customer_type", "name", "first_name", "last_name", "company_name", "contact_name", "contact_email", "phone", "address_line1", "address_line2", "postal_code", "city", "region", "country"] as const;
const camel = (key: string) => key.replace(/_([a-z0-9])/g, (_, c: string) => c.toUpperCase());
type OnboardingStep = "join" | "details" | "passkey" | "done";
const stepLabels: Record<OnboardingStep, string> = { join: "Join", details: "Customer details", passkey: "Passkey", done: "Done" };
/** Where the person is in joining: a new account has already chosen its password. */
function OnboardingProgress({ steps, current, account }: { steps: OnboardingStep[]; current: OnboardingStep; account: boolean }) {
  const all = [...(account ? ["account" as const] : []), ...steps];
  const position = all.indexOf(current);
  const label = (step: (typeof all)[number]) => (step === "account" ? "Account" : stepLabels[step]);
  return (
    <>
    <p className="mb-4 text-center text-xs text-muted sm:hidden">Step {position + 1} of {all.length}: <span className="font-medium text-foreground">{label(current)}</span></p>
    <ol aria-label="Setup progress" className="mb-6 flex flex-wrap items-center justify-center gap-x-2 gap-y-2 text-xs max-sm:hidden">
      {all.map((step, index) => {
        // Reaching the last step completes the whole journey.
        const state = index < position || current === "done" ? "done" : index === position ? "current" : "pending";
        return (
          <li key={step} data-state={state} aria-current={state === "current" ? "step" : undefined} className={`flex items-center gap-2 ${state === "pending" ? "text-muted" : "text-foreground"}`}>
            {index > 0 ? <span aria-hidden className={`h-px w-6 ${state === "pending" ? "bg-outline-strong" : "bg-success"}`} /> : null}
            <span aria-hidden className={`grid size-5 place-items-center rounded-full text-[11px] font-semibold ${{ done: "bg-success text-on-accent", current: "bg-accent text-on-accent", pending: "border border-outline-strong" }[state]}`}>{state === "done" ? "✓" : index + 1}</span>
            <span className={state === "current" ? "font-medium" : undefined}>{label(step)}</span>
          </li>
        );
      })}
    </ol>
    </>
  );
}
/** One focused step: a card with its own footer for the decision. */
function StepCard({ title, lead, children, footer, wide = false }: { title: ReactNode; lead?: ReactNode; children?: ReactNode; footer: ReactNode; wide?: boolean }) {
  return (
    <section className={ui.cx(ui.card, "mx-auto w-full shadow-sm", wide ? "max-w-[720px]" : "max-w-[520px]")}>
      <div className={ui.cx(ui.cardBody, "gap-5 p-8 max-sm:p-6")}>
        <div className="grid gap-2">
          <h1 className="text-2xl font-semibold tracking-[-0.02em]">{title}</h1>
          {lead ? <p className="text-sm leading-relaxed text-muted">{lead}</p> : null}
        </div>
        {children}
      </div>
      <div className={ui.cx(ui.cardFooter, "px-8 max-sm:px-6")}>{footer}</div>
    </section>
  );
}
function InvitationPage() {
  const api = useApi();
  const consoleHref = useConsoleHref();
  const query = new URLSearchParams(location.search);
  const id = query.get("id");
  const welcome = query.get("welcome") === "1";
  const [step, setStep] = useState<OnboardingStep>("join");
  const invite = useQuery({
    queryKey: ["invitation", id],
    queryFn: () =>
      api.request(
        `/api/invitation/${encodeURIComponent(id ?? "")}`,
        z.object({
          organizationName: z.string(),
          email: z.string(),
          role: z.string(),
          status: z.string(),
          customerID: z.string().nullish(),
        }),
        {},
        true,
      ),
  });
  const answer = useMutation({
    mutationFn: (accept: boolean) =>
      api.write(`/api/invitation/${encodeURIComponent(id ?? "")}`, "POST", { accept }, z.object({}), true),
    onSuccess: (_, accept) => {
      if (!accept) { location.assign("/workspaces"); return; }
      setStep(steps[1] ?? "done");
    },
  });
  if (invite.isPending) return <Loading>Loading invitation…</Loading>;
  if (invite.error) return <Failure error={invite.error} />;
  const data = invite.data;
  const manages = Boolean(data.customerID) && ["owner", "admin"].includes(data.role);
  const steps: OnboardingStep[] = ["join", ...(manages ? ["details" as const] : []), ...(welcome && browserSupportsWebAuthn() ? ["passkey" as const] : []), "done"];
  const next = () => { setStep(steps[steps.indexOf(step) + 1] ?? "done"); };
  const open = consoleHref(data.customerID ? `/workspaces/${data.customerID}` : "/tenants");
  return (
    <>
      <OnboardingProgress steps={steps} current={step} account={welcome} />
      {step === "details" && data.customerID ? (
        <CustomerOnboarding customer={data.customerID} organization={data.organizationName} done={next} />
      ) : step === "passkey" ? (
        <WelcomePasskey done={next} />
      ) : step === "done" ? (
        <StepCard
          title="You're all set"
          lead={<>You are now {data.role === "admin" ? "an administrator" : `a${data.role === "owner" ? "n" : ""} ${data.role}`} of <strong className="font-medium text-foreground">{data.organizationName}</strong>. Its projects, applications and databases are waiting in Console.</>}
          footer={<>
            <a className={ui.button("quiet")} href="/account">Account settings</a>
            {open ? <a className={ui.button("primary")} href={open}>Open Console</a> : <a className={ui.button("primary")} href="/workspaces">Your workspaces</a>}
          </>}
        />
      ) : (
        <StepCard
          title={<span className="flex items-center gap-3"><WorkspaceAvatar name={data.organizationName} size="size-11 text-lg" /><span className="min-w-0 [overflow-wrap:anywhere]">Join {data.organizationName}</span></span>}
          lead={<>You were invited as <strong className="font-medium text-foreground">{data.email}</strong>.</>}
          footer={data.status === "pending" ? <>
            <button className={ui.button("quiet")} onClick={() => { answer.mutate(false); }} disabled={answer.isPending}>Decline</button>
            <button className={ui.button("primary")} onClick={() => { answer.mutate(true); }} disabled={answer.isPending}>{answer.isPending ? "Joining…" : "Accept invitation"}</button>
          </> : <>
            <span>This invitation is {data.status === "accepted" ? "already accepted" : "no longer open"}.</span>
            <a className={ui.button()} href="/workspaces">Your workspaces</a>
          </>}
        >
          <div className="flex items-start gap-3 rounded-lg border border-outline bg-subtle px-4 py-3">
            <span className={ui.cx(ui.badge("accent"), "mt-0.5")}>{data.role}</span>
            <p className="text-[13px] text-muted">{roleText[data.role] ?? "You get the access this role allows."}</p>
          </div>
          <Feedback error={answer.error} />
        </StepCard>
      )}
    </>
  );
}
/** After joining, an owner or administrator confirms the customer details or fills in what is missing. */
function CustomerOnboarding({ customer, organization, done }: { customer: string; organization: string; done: () => void }) {
  const api = useApi();
  const live = useLiveValidation();
  const tenant = useQuery({
    queryKey: ["tenant", customer],
    queryFn: () => api.request(`/api/tenant/${encodeURIComponent(customer)}`, z.object({ tenant: z.record(z.string(), z.unknown()) }), {}, true),
  });
  const save = useMutation({
    mutationFn: (data: FormData) => {
      const value = (key: string) => formText(data, camel(key)).trim() || null;
      const type = formText(data, "customerType") === "person" ? "person" : "company";
      const name = type === "company" ? value("company_name") : [value("first_name"), value("last_name")].filter(Boolean).join(" ");
      const profile = Object.fromEntries(profileKeys.filter((key) => !["customer_type", "name"].includes(key)).map((key) => [key, value(key)]));
      return api.write(`/api/tenant/${encodeURIComponent(customer)}`, "POST", { action: "profile", profile: { ...profile, customer_type: type, name } }, z.object({}), true);
    },
    onSuccess: done,
  });
  if (tenant.isPending) return <Loading>Loading customer details…</Loading>;
  if (tenant.error) return <Failure error={tenant.error} />;
  const record = { id: customer, ...Object.fromEntries(profileKeys.map((key) => [camel(key), tenant.data.tenant[key] ?? null])) } as RegistryRecord;
  return (
    <form className="contents" {...live.form} onSubmit={(event) => { event.preventDefault(); save.mutate(new FormData(event.currentTarget)); }}>
      <StepCard
        wide
        title="Confirm your customer details"
        lead={<>These details belong to <strong className="font-medium text-foreground">{organization}</strong> and appear on invoices. Check what was prepared for you and fill in anything missing; you can change them later.</>}
        footer={<>
          <button type="button" className={ui.button("quiet")} disabled={save.isPending} onClick={done}>Later</button>
          <button className={ui.button("primary")} disabled={save.isPending}>{save.isPending ? "Saving…" : "Confirm details"}</button>
        </>}
      >
        <div className="grid gap-6">
          <CustomerFields record={record} pending={save.isPending} errors={live.errors} />
        </div>
        <Feedback error={save.error} />
      </StepCard>
    </form>
  );
}
function WelcomePasskey({ done }: { done: () => void }) {
  const api = useApi();
  const add = useMutation({
    mutationFn: async () => {
      const options = await api.request(
        "/api/auth/passkey/generate-register-options",
        z.custom<PublicKeyCredentialCreationOptionsJSON>((value) => typeof value === "object" && value !== null && "challenge" in value),
        {},
        true,
      );
      const response = await startRegistration({ optionsJSON: options });
      return api.write("/api/auth/passkey/verify-registration", "POST", { response, name: "My passkey" }, z.object({ id: z.string() }), true);
    },
    onSuccess: done,
  });
  return (
    <StepCard
      title={<span className="flex items-center gap-3"><span aria-hidden className="grid size-11 shrink-0 place-items-center rounded-full bg-soft text-foreground"><FingerprintIcon size={24} /></span>Add a passkey</span>}
      lead="Sign in with your fingerprint, face or device PIN instead of typing your password."
      footer={<>
        <button className={ui.button("quiet")} disabled={add.isPending} onClick={done}>Not now</button>
        <button className={ui.button("primary")} disabled={add.isPending} onClick={() => { add.mutate(); }}>{add.isPending ? "Waiting for your device…" : "Add passkey"}</button>
      </>}
    >
      <ul className="grid gap-2 text-[13px] text-muted">
        <li className="flex gap-2"><ShieldCheckIcon size={18} aria-hidden className="shrink-0 text-success" />Phishing-resistant: it only works on this site.</li>
        <li className="flex gap-2"><DevicesIcon size={18} aria-hidden className="shrink-0 text-success" />Stays on your device; add more devices later in Account security.</li>
      </ul>
      <Feedback error={add.error} />
    </StepCard>
  );
}
export function AccountApp() {
  useEffect(() => {
    const previous = document.title;
    document.title = "Spitzli Account";
    return () => { document.title = previous; };
  }, []);
  const api = useApi();
  const client = useQueryClient();
  const [confirmingMfa, setConfirmingMfa] = useState(false);
  const confirmation = useRef<AbortController | null>(null);
  useEffect(
    () => () => {
      confirmation.current?.abort();
    },
    [],
  );
  const session = useQuery({
    queryKey: ["account-session"],
    queryFn: ({ signal }) =>
      api.request(
        "/api/auth/get-session",
        nativeSessionSchema,
        { signal },
        true,
      ),
    staleTime: 0,
    refetchInterval: confirmingMfa ? false : 60000,
    refetchOnWindowFocus: !confirmingMfa,
    refetchOnReconnect: !confirmingMfa,
  });
  const cacheReady = useSessionCacheIsolation(
    session.isError ? null : session.data,
    session.dataUpdatedAt,
  );
  // Only same-owner completion text and unacknowledged one-time codes survive
  // the private-cache barrier. Queries, forms and enrollment setup still unmount.
  const feedback = useAccountFeedback(
    session.isError || !session.isFetchedAfterMount ? null : session.data,
  );
  const continuation = authorizationContinuation(
    new URLSearchParams(location.search).get("returnTo"),
    location.origin,
  );
  const [reauthenticatedScope, setReauthenticatedScope] = useState<
    string | null
  >(null);
  const reauthenticated =
    reauthenticatedScope ===
    sessionCacheScope(session.data, session.dataUpdatedAt);
  const forceLogin =
    !!continuation &&
    (() => {
      const q = new URL(continuation, location.origin).searchParams;
      return q.get("prompt") === "login" || q.has("max_age");
    })();
  const resume =
    continuation && reauthenticated && forceLogin
      ? (() => {
          const u = new URL(continuation, location.origin);
          u.searchParams.delete("prompt");
          u.searchParams.delete("max_age");
          return u.pathname + u.search;
        })()
      : continuation;
  const signedIn = () => {
    void client
      .invalidateQueries({ queryKey: ["account-session"] })
      .then(() => {
        const confirmed = client.getQueryState<Session | null>([
          "account-session",
        ]);
        if (confirmed?.status === "success" && confirmed.data)
          setReauthenticatedScope(
            sessionCacheScope(confirmed.data, confirmed.dataUpdatedAt),
          );
      });
  };

  const confirmEnrollment = async (code: string, codes: string[]) => {
    const before = session.data;
    if (!before || confirmation.current)
      throw new Error("Refresh your account before continuing.");
    const controller = new AbortController();
    confirmation.current = controller;
    setConfirmingMfa(true);
    feedback.setConfirmationError(null);
    const expected = sessionCacheScope(before);
    const currentAuthority = () => {
      const current = client.getQueryState<Session | null>(["account-session"]);
      return (
        current?.status === "success" &&
        sessionCacheScope(current.data) === expected
      );
    };
    const abort = new Promise<never>((_resolve, reject) => {
      controller.signal.addEventListener(
        "abort",
        () => {
          reject(new Error("Account verification was interrupted."));
        },
        { once: true },
      );
    });
    const timer = setTimeout(() => {
      controller.abort();
    }, 15000);
    const sessionQuery = client
      .getQueryCache()
      .find({ queryKey: ["account-session"], exact: true });
    const unsubscribe = client.getQueryCache().subscribe((event) => {
      if (event.query === sessionQuery && !currentAuthority())
        controller.abort();
    });
    const verify = async () => {
      await client.cancelQueries({
        queryKey: ["account-session"],
        exact: true,
      });
      if (!currentAuthority() || controller.signal.aborted)
        throw new Error("Account authority changed.");
      await api.request(
        "/api/auth/two-factor/verify-totp",
        resultSchema,
        {
          method: "POST",
          body: JSON.stringify({ code }),
          signal: controller.signal,
        },
        true,
      );
      const confirmed = await api.request(
        "/api/auth/get-session",
        nativeSessionSchema,
        { signal: controller.signal },
        true,
      );
      if (!confirmedMfaSession(before, confirmed))
        throw new Error("The current account could not be verified.");
      return confirmed;
    };
    try {
      const confirmed = await Promise.race([verify(), abort]);
      if (!currentAuthority() || controller.signal.aborted)
        throw new Error("Account authority changed.");
      unsubscribe();
      // Publish only this explicitly verified authentication transition, never an arbitrary poll.
      client.setQueryData(["account-session"], confirmed);
      feedback.adoptConfirmedCodes(confirmed, codes);
      setReauthenticatedScope(sessionCacheScope(confirmed));
    } catch {
      feedback.setConfirmationError(
        "The authenticator may be enabled, but the current session could not be verified. Refresh your account before continuing.",
      );
      throw new Error("Authenticator confirmation could not be verified.");
    } finally {
      unsubscribe();
      clearTimeout(timer);
      controller.abort();
      confirmation.current = null;
      setConfirmingMfa(false);
    }
  };

  const ready =
    !confirmingMfa &&
    !feedback.confirmationError &&
    feedback.codes === null &&
    cacheReady &&
    session.isFetchedAfterMount &&
    !session.isError &&
    accountCanContinue(session.data);
  const refresh = () => {
    void client.invalidateQueries({ queryKey: ["account-session"] });
    void client.invalidateQueries({ queryKey: ["account-sessions"] });
  };
  useEffect(() => {
    if (ready && resume && (!forceLogin || reauthenticated))
      location.replace(resume);
  }, [ready, resume, forceLogin, reauthenticated]);
  if (!cacheReady) return <PageProgress product="Account">Opening your account…</PageProgress>;
  if (location.pathname === "/reset-password") return <PasswordReset />;
  if (location.pathname === "/auth-error")
    return (
      <main className={authFrame}>
        <AuthHeader product="Account" href="/account" />
        <section className={authPanel}>
          <h1>This application could not be opened</h1>
          <p>
            Your sign-in request may have expired, or this account may not have
            access.
          </p>
          <a className={ui.cx(`${ui.button("primary")} mt-5 w-full`)} href="/account">
            Open your account
          </a>
          <a className={ui.textButton} href="/logout">
            Sign out to use another account
          </a>
        </section>
      </main>
    );
  if (session.isPending || (!session.isFetchedAfterMount && !session.isError))
    return <PageProgress product="Account">Opening your account…</PageProgress>;
  if (session.error)
    return (
      <PageFailure
        product="Account"
        title="Account unavailable"
        error={session.error}
        retry={() => {
          void session.refetch();
        }}
      />
    );
  const ownedSetup = canFinishOwnedSetup(session.data, feedback);
  if (!session.data || (forceLogin && !reauthenticated && !ownedSetup))
    return <AccountSignIn onSuccess={signedIn} />;
  if (continuation && ready)
    return <PageProgress product="Account">Returning to your application…</PageProgress>;
  const content =
    ready && location.pathname === "/consent" ? (
      <ConsentPage />
    ) : location.pathname === "/logout" ? (
      <LogoutPage session={session.data} />
    ) : ready && ["/workspaces", "/sites", "/developers"].includes(location.pathname) ? (
      // Older mails link to /sites; websites now live inside a workspace's projects.
      location.pathname === "/developers" ? <DeveloperAccess /> : <WorkspacesPage />
    ) : ready && location.pathname === "/invitation" ? (
      <InvitationPage />
    ) : (
      <>
        <div className={ui.pageHeading}>
          <div>
            <h1 className={ui.pageTitle}>Account security</h1>
            <p className={ui.pageLead}>Choose how you sign in and manage access to your account.</p>
          </div>
          {ready && api.config.consoleOrigin ? (
            <a
              className={ui.button("primary")}
              href={`${api.config.consoleOrigin}/api/sso/login`}
            >
              Open Console
            </a>
          ) : null}
        </div>
        {session.data.user.preview ? (
          <section className={ui.card}>
            <div className={ui.cardBody}>
              <h2 className={ui.sectionTitle}>Customer preview is read-only</h2>
              <p className="text-muted">
                Exit your customer preview in Console before changing account
                security.
              </p>
            </div>
            <div className={ui.cx(`${ui.cardFooter} justify-end`)}>
              <a
                className={ui.button()}
                href={`${api.config.consoleOrigin ?? ""}/projects`}
              >
                Return to Console
              </a>
            </div>
          </section>
        ) : (
          <AccountSecurity
            session={session.data}
            refresh={refresh}
            feedback={feedback}
            confirmEnrollment={confirmEnrollment}
          />
        )}
      </>
    );
  return (
    <AccountShell session={session.data} consoleLink={ready}>
      {content}
    </AccountShell>
  );
}

function AccountShell({ session, consoleLink, children }: { session: Session; consoleLink: boolean; children: ReactNode }) {
  const api = useApi();
  const consoleHref = consoleLink && api.config.consoleOrigin ? `${api.config.consoleOrigin}/api/sso/login` : undefined;
  const focused = ["/consent", "/logout", "/invitation"].includes(location.pathname);
  return focused ? (
    <main className={authFrame}>
      <AuthHeader product="Account" href="/account" />
      <div className={location.pathname === "/invitation" ? "w-full max-w-[760px]" : "w-full max-w-[540px]"}>{children}</div>
    </main>
  ) : (
    <div className="account-layout">
      <a className="fixed -top-24 left-4 z-50 border border-focus bg-surface px-4 py-2.5 focus:top-2.5" href="#main">Skip to content</a>
      <header className="account-header">
        <div className="account-header-inner">
          <AccountBrand />
          <div className="ml-auto flex items-center gap-1">
            {consoleHref ? <a className="account-console-link" href={consoleHref}>Console <span aria-hidden>↗</span></a> : null}
            <LocalePicker />
            <ThemeButton />
            <UserMenu name={session.user.name} email={session.user.email} consoleHref={consoleHref} signOut="/logout" />
          </div>
        </div>
      </header>
      <div className="account-navigation">
        <nav aria-label="Account" className="account-navigation-inner">
          {[
            { href: "/account", label: "Security", Icon: ShieldCheckIcon },
            { href: "/workspaces", label: "Workspaces", Icon: BuildingsIcon },
            { href: "/developers", label: "Developer access", Icon: KeyIcon },
          ].map(({ href, label, Icon }) => (
            <a key={href} href={href} aria-current={location.pathname === href || (href === "/workspaces" && location.pathname === "/sites") ? "page" : undefined}>
              <Icon size={18} aria-hidden />{label}
            </a>
          ))}
        </nav>
      </div>
      <main id="main" tabIndex={-1} className="account-content">
        <div className="account-identity">
          <span className="account-person" aria-hidden>{Array.from(session.user.name.trim() || session.user.email)[0]?.toUpperCase()}</span>
          <div className="min-w-0"><p className="account-context">Your Spitzli account</p><p className="account-person-name">{session.user.name}</p><p className="account-person-email">{session.user.email}</p></div>
        </div>
        {children}
      </main>
      <footer className="account-footer">One Spitzli account. All your workspaces.</footer>
    </div>
  );
}
