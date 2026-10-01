import { X } from "lucide-react";

import type { Translator } from "../i18n";
import type { CommandError } from "../lib/types";

interface ErrorBannerProps {
  error: CommandError;
  onDismiss: () => void;
  t: Translator;
}

/**
 * Errors are shown as a localised sentence plus a stable code. The raw OS error
 * never reaches the user: it stays in the developer log, exactly as the
 * project's error policy requires.
 */
export function ErrorBanner({ error, onDismiss, t }: ErrorBannerProps) {
  const message = t(`error.${error.code}`);
  const translationMissing = message === `error.${error.code}`;

  return (
    <div
      className="flex items-start gap-2 border-b border-[var(--lce-border)] px-4 py-2 text-[12px]"
      style={{ background: "var(--lce-danger-soft)", color: "var(--lce-danger)" }}
      role="alert"
    >
      <span className="flex-1">
        <strong>{t("error.title")}</strong>{" "}
        {translationMissing ? error.message : message}
        <span className="ml-2 opacity-70">({error.code})</span>
      </span>
      <button
        type="button"
        className="lce-chip"
        onClick={onDismiss}
        aria-label={t("error.dismiss")}
      >
        <X size={12} aria-hidden />
      </button>
    </div>
  );
}