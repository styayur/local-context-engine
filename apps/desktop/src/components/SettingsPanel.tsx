import { HardDriveDownload, RefreshCw, Trash2, X } from "lucide-react";
import { useState } from "react";

import type { Translator } from "../i18n";
import type {
  IndexBackend,
  IndexStatus,
  Language,
  Settings,
  Theme,
} from "../lib/types";

interface SettingsPanelProps {
  settings: Settings;
  indexStatus: IndexStatus | null;
  rebuilding: boolean;
  onChange: (patch: Partial<Settings>) => void;
  onRebuildIndex: () => void;
  onResetUsage: () => void;
  onClose: () => void;
  t: Translator;
}

type Tab = "general" | "search" | "index" | "language" | "keyboard" | "appearance" | "about";

const TABS: { id: Tab; labelKey: string }[] = [
  { id: "general", labelKey: "settings.general" },
  { id: "search", labelKey: "settings.search" },
  { id: "index", labelKey: "settings.index" },
  { id: "language", labelKey: "settings.language" },
  { id: "keyboard", labelKey: "settings.keyboard" },
  { id: "appearance", labelKey: "settings.appearance" },
  { id: "about", labelKey: "settings.about" },
];

/** Every user-facing setting, grouped into tabs. */
export function SettingsPanel({
  settings,
  indexStatus,
  rebuilding,
  onChange,
  onRebuildIndex,
  onResetUsage,
  onClose,
  t,
}: SettingsPanelProps) {
  const [tab, setTab] = useState<Tab>("general");

  return (
    <div className="lce-overlay" role="dialog" aria-modal="true" aria-label={t("settings.title")}>
      <div className="lce-card">
        <header className="flex items-center justify-between border-b border-[var(--lce-border)] px-5 py-3">
          <h2 className="text-[15px] font-semibold">{t("settings.title")}</h2>
          <button
            type="button"
            className="lce-chip"
            onClick={onClose}
            aria-label={t("settings.close")}
          >
            <X size={13} aria-hidden />
          </button>
        </header>

        <div className="flex min-h-0 flex-1">
          <nav
            className="w-40 shrink-0 border-r border-[var(--lce-border)] p-2"
            aria-label={t("settings.title")}
          >
            {TABS.map((entry) => (
              <button
                key={entry.id}
                type="button"
                className="lce-menu-item"
                style={
                  tab === entry.id
                    ? { background: "var(--lce-accent-soft)", color: "var(--lce-fg)" }
                    : undefined
                }
                onClick={() => setTab(entry.id)}
                aria-current={tab === entry.id}
              >
                {t(entry.labelKey)}
              </button>
            ))}
          </nav>

          <div className="lce-scroll min-h-0 flex-1 p-5">
            {tab === "general" && (
              <Section title={t("settings.general")}>
                <Row label={t("settings.resultLimit")}>
                  <input
                    type="number"
                    min={1}
                    max={2000}
                    className="lce-chip"
                    style={{ width: 90 }}
                    value={settings.resultLimit}
                    onChange={(event) =>
                      onChange({ resultLimit: Number(event.target.value) || 1 })
                    }
                  />
                </Row>
                <Row label={t("settings.includeDirectories")}>
                  <Toggle
                    checked={settings.includeDirectories}
                    onChange={(value) => onChange({ includeDirectories: value })}
                  />
                </Row>
              </Section>
            )}

            {tab === "search" && (
              <Section title={t("settings.search")}>
                <Row label={t("settings.fuzzy")}>
                  <Toggle
                    checked={settings.fuzzyMatching}
                    onChange={(value) => onChange({ fuzzyMatching: value })}
                  />
                </Row>
                <Row label={t("settings.usageRanking")}>
                  <Toggle
                    checked={settings.usageRanking}
                    onChange={(value) => onChange({ usageRanking: value })}
                  />
                </Row>
                <Row label="">
                  <button type="button" className="lce-chip" onClick={onResetUsage}>
                    <Trash2 size={12} aria-hidden />
                    {t("settings.resetUsage")}
                  </button>
                </Row>
              </Section>
            )}

            {tab === "index" && (
              <Section title={t("settings.index")}>
                <Row label={t("settings.indexBackend")}>
                  <select
                    className="lce-chip"
                    value={settings.indexBackend}
                    onChange={(event) =>
                      onChange({ indexBackend: event.target.value as IndexBackend })
                    }
                  >
                    <option value="auto">{t("settings.backend.auto")}</option>
                    <option value="scan">{t("settings.backend.scan")}</option>
                    <option value="mft-usn">{t("settings.backend.mft-usn")}</option>
                  </select>
                </Row>
                <Row label={t("settings.maxEntries")}>
                  <input
                    type="number"
                    min={1000}
                    max={20000000}
                    step={1000}
                    className="lce-chip"
                    style={{ width: 120 }}
                    value={settings.maxIndexEntries}
                    onChange={(event) =>
                      onChange({ maxIndexEntries: Number(event.target.value) || 1000 })
                    }
                  />
                </Row>
                <Row label={t("settings.indexStatus")}>
                  <span>
                    {indexStatus?.ready
                      ? t("settings.indexReady")
                      : t("settings.indexNotBuilt")}
                  </span>
                </Row>
                {indexStatus && (
                  <>
                    <Row label={t("settings.indexBackendActive")}>
                      <span>{indexStatus.backend}</span>
                    </Row>
                    <Row label={t("settings.indexEntries")}>
                      <span>
                        {indexStatus.entries} ({indexStatus.files} / {indexStatus.directories})
                      </span>
                    </Row>
                    <Row label={t("settings.indexMemory")}>
                      <span>{formatBytes(indexStatus.memoryBytes)}</span>
                    </Row>
                    <Row label={t("settings.indexVolumes")}>
                      <span>{indexStatus.volumes.join(", ") || "—"}</span>
                    </Row>
                    <Row label={t("settings.indexCache")}>
                      <code className="truncate" style={{ maxWidth: 280 }}>
                        {indexStatus.cachePath}
                      </code>
                    </Row>
                  </>
                )}
                <Row label="">
                  <button
                    type="button"
                    className="lce-chip"
                    disabled={rebuilding}
                    onClick={onRebuildIndex}
                  >
                    {rebuilding ? (
                      <RefreshCw size={12} className="animate-spin" aria-hidden />
                    ) : (
                      <HardDriveDownload size={12} aria-hidden />
                    )}
                    {rebuilding ? t("search.building") : t("settings.rebuildIndex")}
                  </button>
                </Row>
                {indexStatus?.providers.map((provider) => (
                  <Row key={provider.name} label={provider.name}>
                    <span className="text-[var(--lce-muted)]">
                      {t(`providers.scope.${provider.scope}`)}
                      {provider.entity_count !== null ? ` · ${provider.entity_count}` : ""}
                    </span>
                  </Row>
                ))}
              </Section>
            )}

            {tab === "language" && (
              <Section title={t("settings.language")}>
                {(["system", "zh-CN", "en-US"] as Language[]).map((value) => (
                  <label key={value} className="lce-menu-item" style={{ cursor: "pointer" }}>
                    <input
                      type="radio"
                      name="language"
                      checked={settings.language === value}
                      onChange={() => onChange({ language: value })}
                    />
                    <span>{t(`settings.language.${value}`)}</span>
                  </label>
                ))}
              </Section>
            )}

            {tab === "keyboard" && (
              <Section title={t("settings.keyboard")}>
                <Row label={t("settings.hotkey")}>
                  <input
                    type="text"
                    className="lce-chip"
                    style={{ width: 150 }}
                    value={settings.hotkey}
                    onChange={(event) => onChange({ hotkey: event.target.value })}
                  />
                </Row>
                <p className="text-[var(--lce-muted)]">{t("settings.hotkey.hint")}</p>
                <HotkeyTable t={t} />
              </Section>
            )}

            {tab === "appearance" && (
              <Section title={t("settings.theme")}>
                {(["system", "light", "dark"] as Theme[]).map((value) => (
                  <label key={value} className="lce-menu-item" style={{ cursor: "pointer" }}>
                    <input
                      type="radio"
                      name="theme"
                      checked={settings.theme === value}
                      onChange={() => onChange({ theme: value })}
                    />
                    <span>{t(`settings.theme.${value}`)}</span>
                  </label>
                ))}
              </Section>
            )}

            {tab === "about" && (
              <Section title={t("app.name")}>
                <p>{t("settings.about.body")}</p>
                <p className="mt-3 text-[var(--lce-muted)]">
                  {t("settings.about.version", { version: __APP_VERSION__ })}
                </p>
              </Section>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-3">
      <h3 className="text-[12px] uppercase tracking-wide text-[var(--lce-muted)]">{title}</h3>
      {children}
    </section>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4">
      <span>{label}</span>
      {children}
    </div>
  );
}

function Toggle({
  checked,
  onChange,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <input
      type="checkbox"
      checked={checked}
      onChange={(event) => onChange(event.target.checked)}
      style={{ width: 16, height: 16 }}
    />
  );
}

function HotkeyTable({ t }: { t: Translator }) {
  const rows: [string, string][] = [
    ["↑ ↓", t("hotkey.navigate")],
    ["Enter", t("hotkey.open")],
    ["Shift+Enter", t("action.open")],
    ["Tab", t("hotkey.filters")],
    ["Ctrl+,", t("hotkey.settings")],
    ["Esc", t("hotkey.close")],
  ];
  return (
    <table className="mt-2 w-full text-[12px]">
      <tbody>
        {rows.map(([keys, label]) => (
          <tr key={keys}>
            <td className="w-28 py-0.5">
              <code>{keys}</code>
            </td>
            <td className="py-0.5 text-[var(--lce-muted)]">{label}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${bytes} B` : `${value.toFixed(1)} ${units[unit]}`;
}