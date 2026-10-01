import { AlertTriangle } from "lucide-react";
import { useEffect, useRef } from "react";

import type { Translator } from "../i18n";

interface ConfirmDialogProps {
  title: string;
  body: string;
  confirmLabel: string;
  destructive?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  t: Translator;
}

/**
 * The gate in front of every destructive action.
 *
 * Focus starts on the cancel button, so pressing Enter out of habit cannot
 * confirm a process termination.
 */
export function ConfirmDialog({
  title,
  body,
  confirmLabel,
  destructive,
  onConfirm,
  onCancel,
  t,
}: ConfirmDialogProps) {
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    cancelRef.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onCancel();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return (
    <div className="lce-overlay" role="dialog" aria-modal="true" aria-label={title}>
      <div className="lce-card" style={{ maxWidth: 460 }}>
        <div className="flex items-start gap-3 p-5">
          <AlertTriangle
            size={20}
            className="mt-0.5 shrink-0"
            style={{ color: destructive ? "var(--lce-danger)" : "var(--lce-accent)" }}
            aria-hidden
          />
          <div>
            <h2 className="text-[15px] font-semibold">{title}</h2>
            <p className="mt-1 text-[var(--lce-muted)]">{body}</p>
          </div>
        </div>
        <div className="flex justify-end gap-2 border-t border-[var(--lce-border)] px-5 py-3">
          <button ref={cancelRef} type="button" className="lce-chip" onClick={onCancel}>
            {t("confirm.cancel")}
          </button>
          <button
            type="button"
            className="lce-chip"
            data-active="true"
            style={
              destructive
                ? { color: "var(--lce-danger)", borderColor: "var(--lce-danger)" }
                : undefined
            }
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}