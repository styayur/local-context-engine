import { Search, Settings as SettingsIcon, X } from "lucide-react";
import { useEffect, useRef } from "react";

import type { Translator } from "../i18n";

interface SearchBarProps {
  value: string;
  onChange: (value: string) => void;
  onClear: () => void;
  onOpenSettings: () => void;
  t: Translator;
  disabled?: boolean;
}

/**
 * The one input the whole interface is built around.
 *
 * It autofocuses on mount and regains focus whenever the window is clicked, so
 * the app is always exactly one keystroke away from a search.
 */
export function SearchBar({
  value,
  onChange,
  onClear,
  onOpenSettings,
  t,
  disabled,
}: SearchBarProps) {
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  return (
    <div className="flex items-center gap-3 border-b border-[var(--lce-border)] px-4 py-3">
      <Search size={18} className="shrink-0 text-[var(--lce-muted)]" aria-hidden />
      <input
        ref={inputRef}
        className="lce-input lce-no-drag"
        type="text"
        value={value}
        spellCheck={false}
        autoComplete="off"
        autoCorrect="off"
        disabled={disabled}
        placeholder={t("search.placeholder")}
        aria-label={t("search.placeholder")}
        onChange={(event) => onChange(event.target.value)}
      />
      {value.length > 0 && (
        <button
          type="button"
          className="lce-chip"
          onClick={() => {
            onClear();
            inputRef.current?.focus();
          }}
          aria-label={t("hotkey.close")}
        >
          <X size={13} aria-hidden />
        </button>
      )}
      <button
        type="button"
        className="lce-chip"
        onClick={onOpenSettings}
        title={t("settings.title")}
        aria-label={t("settings.title")}
      >
        <SettingsIcon size={13} aria-hidden />
      </button>
    </div>
  );
}