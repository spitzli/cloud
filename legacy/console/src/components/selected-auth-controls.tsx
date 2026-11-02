"use client";
import { useClerk } from "@clerk/nextjs";
import { useI18n } from "@webdock/i18n/react";
export function SelectedAuthControls() {
  const { signOut } = useClerk();
  const i18n = useI18n();
  return (
    <button
      type="button"
      className="text-button"
      onClick={() => void signOut({ redirectUrl: "/sign-in" })}
    >
      {i18n.t("Sign out")}
    </button>
  );
}
