import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { ContextMenu } from "./components/ContextMenu";
import { ConfirmDialog } from "./components/ConfirmDialog";
import { EmptyState } from "./components/EmptyState";
import { ErrorBanner } from "./components/ErrorBanner";
import { ResultList } from "./components/ResultList";
import { SearchBar } from "./components/SearchBar";
import { SettingsPanel } from "./components/SettingsPanel";
import { StatusBar } from "./components/StatusBar";
import { TypeFilter } from "./components/TypeFilter";
import { useKeyboardNavigation } from "./hooks/useKeyboardNavigation";
import { useSearch } from "./hooks/useSearch";
import { createTranslator, resolveLocale, type LanguagePreference } from "./i18n";
import {
  actionsFor,
  clipboardTextFor,
  pidOf,
  type ActionId,
  type ContextAction,
} from "./lib/actions";
import * as api from "./lib/tauri";
import {
  asCommandError,
  FILTER_TYPES,
  type CommandError,
  type EntityType,
  type IndexStatus,
  type SearchResult,
  type Settings,
} from "./lib/types";

/** The filter values, in the order `Tab` cycles through them. */
const FILTER_KEYS = FILTER_TYPES.map((filter) => filter.key);

const FALLBACK_SETTINGS: Settings = {
  language: "system",
  theme: "system",
  resultLimit: 50,
  fuzzyMatching: true,
  usageRanking: true,
  indexBackend: "auto",
  indexedDrives: [],
  maxIndexEntries: 1_000_000,
  includeDirectories: true,
  hotkey: "Alt+Space",
  includeHiddenWindows: false,
};

interface PendingConfirmation {
  title: string;
  body: string;
  confirmLabel: string;
  destructive: boolean;
  run: () => Promise<void>;
}

export default function App() {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<"all" | EntityType>("all");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [settings, setSettings] = useState<Settings>(FALLBACK_SETTINGS);
  const [indexStatus, setIndexStatus] = useState<IndexStatus | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [menu, setMenu] = useState<{
    actions: ContextAction[];
    position: { x: number; y: number };
    result: SearchResult;
  } | null>(null);
  const [confirmation, setConfirmation] = useState<PendingConfirmation | null>(null);
  const [actionError, setActionError] = useState<CommandError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const shellAvailable = useRef(api.isDesktopShell());

  const locale = resolveLocale(settings.language as LanguagePreference);
  const t = useMemo(() => createTranslator(locale), [locale]);

  const types = useMemo(
    () => (filter === "all" ? [] : [filter]),
    [filter],
  );

  const refreshIndexStatus = useCallback(() => {
    if (!shellAvailable.current) return;
    api
      .getIndexStatus()
      .then(setIndexStatus)
      .catch(() => setIndexStatus(null));
  }, []);

  const { state, response, error, rebuilding, rebuild, clearError } = useSearch(
    query,
    types,
    settings.resultLimit,
    shellAvailable.current,
    refreshIndexStatus,
  );

  const results = response?.results ?? [];

  // --- theme -----------------------------------------------------------
  useEffect(() => {
    const root = document.documentElement;
    const apply = () => {
      const dark =
        settings.theme === "dark" ||
        (settings.theme === "system" &&
          window.matchMedia("(prefers-color-scheme: dark)").matches);
      root.classList.toggle("dark", dark);
    };
    apply();
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [settings.theme]);

  // --- load settings ----------------------------------------------------
  useEffect(() => {
    if (!shellAvailable.current) return;
    api
      .getSettings()
      .then((loaded) => setSettings(loaded))
      .catch((cause: unknown) => setActionError(asCommandError(cause)));
    refreshIndexStatus();
  }, [refreshIndexStatus]);

  // --- selection --------------------------------------------------------
  useEffect(() => {
    setSelectedIndex((current) => {
      if (results.length === 0) return 0;
      return Math.min(current, results.length - 1);
    });
  }, [results.length]);

  useEffect(() => {
    setSelectedIndex(0);
  }, [query, filter]);

  const showNotice = useCallback((message: string) => {
    setNotice(message);
    window.setTimeout(() => setNotice((current) => (current === message ? null : current)), 1600);
  }, []);

  const persistSettings = useCallback((patch: Partial<Settings>) => {
    setSettings((current) => {
      const next = { ...current, ...patch };
      if (shellAvailable.current) {
        api
          .updateSettings(next)
          .then(setSettings)
          .catch((cause: unknown) => setActionError(asCommandError(cause)));
      }
      return next;
    });
  }, []);

  const activate = useCallback(
    (result: SearchResult | undefined) => {
      if (!result) return;
      api
        .openResult(result.entity)
        .then(() => api.recordSelection(query, result.id))
        .catch((cause: unknown) => setActionError(asCommandError(cause)));
    },
    [query],
  );

  const runAction = useCallback(
    (action: ActionId, result: SearchResult) => {
      setMenu(null);
      const fail = (cause: unknown) => setActionError(asCommandError(cause));
      switch (action) {
        case "open":
          activate(result);
          return;
        case "reveal":
          api.revealResult(result.entity).catch(fail);
          return;
        case "remember":
          api
            .recordSelection(query, result.id)
            .then(() => showNotice(t("action.recordSelection")))
            .catch(fail);
          return;
        case "terminate": {
          const pid = pidOf(result);
          if (pid === null) return;
          // The only destructive action in the product. It runs through a
          // modal whose default focus is Cancel, and only then through the
          // Rust side, which refuses `confirmed = false` anyway.
          setConfirmation({
            title: t("confirm.terminate.title"),
            body: t("confirm.terminate.body", { name: result.display_name, pid }),
            confirmLabel: t("confirm.terminate.confirm"),
            destructive: true,
            run: () => api.terminateProcess(pid, true),
          });
          return;
        }
        default: {
          const text = clipboardTextFor(action, result);
          if (!text) return;
          api
            .copyToClipboard(text)
            .then(() => showNotice(t("action.copied")))
            .catch(fail);
        }
      }
    },
    [activate, query, showNotice, t],
  );

  // --- keyboard ---------------------------------------------------------
  const handlers = useMemo(
    () => ({
      enabled: !settingsOpen && confirmation === null && menu === null,
      onMove: (delta: number) =>
        setSelectedIndex((current) => {
          if (results.length === 0) return 0;
          const next = current + delta;
          if (next < 0) return results.length - 1;
          if (next >= results.length) return 0;
          return next;
        }),
      onActivate: () => activate(results[selectedIndex]),
      onEscape: () => {
        if (query.length > 0) setQuery("");
        else window.close();
      },
      onCycleFilter: (delta: number) => {
        const index = FILTER_KEYS.indexOf(filter);
        const next = (index + delta + FILTER_KEYS.length) % FILTER_KEYS.length;
        const key = FILTER_KEYS[next];
        if (key) setFilter(key);
      },
      onSettings: () => setSettingsOpen(true),
      onContextMenu: () => {
        const result = results[selectedIndex];
        if (!result) return;
        setMenu({
          actions: actionsFor(result),
          position: { x: 220, y: 160 },
          result,
        });
      },
    }),
    [activate, confirmation, filter, menu, query, results, selectedIndex, settingsOpen],
  );

  useKeyboardNavigation(handlers);

  // --- counts for the filter chips --------------------------------------
  const counts = useMemo(() => {
    const map: Partial<Record<"all" | EntityType, number>> = {};
    if (!response) return map;
    map.all = response.results.length;
    for (const result of response.results) {
      map[result.entity_type] = (map[result.entity_type] ?? 0) + 1;
    }
    return map;
  }, [response]);

  const showEmpty = query.trim().length > 0 && state !== "searching" && results.length === 0;

  return (
    <div className="lce-shell">
      <div className="lce-drag-region h-2" />

      <SearchBar
        value={query}
        onChange={setQuery}
        onClear={() => setQuery("")}
        onOpenSettings={() => setSettingsOpen(true)}
        t={t}
      />

      <TypeFilter active={filter} onChange={setFilter} counts={counts} t={t} />

      {(error ?? actionError) && (
        <ErrorBanner
          error={(error ?? actionError)!}
          onDismiss={() => {
            clearError();
            setActionError(null);
          }}
          t={t}
        />
      )}

      {!shellAvailable.current ? (
        <div className="flex h-full items-center justify-center px-8 text-center text-[var(--lce-muted)]">
          {t("error.unsupported")} — run <code className="mx-1">npm run tauri:dev</code>
        </div>
      ) : showEmpty ? (
        <EmptyState
          mode="empty"
          indexReady={indexStatus?.ready ?? true}
          rebuilding={rebuilding}
          onBuildIndex={rebuild}
          t={t}
        />
      ) : query.trim().length === 0 ? (
        <EmptyState
          mode="idle"
          indexReady={indexStatus?.ready ?? true}
          rebuilding={rebuilding}
          onBuildIndex={rebuild}
          t={t}
        />
      ) : (
        <ResultList
          results={results}
          selectedIndex={selectedIndex}
          onSelect={setSelectedIndex}
          onActivate={activate}
          onContextMenu={(result, position) =>
            setMenu({ actions: actionsFor(result), position, result })
          }
          t={t}
        />
      )}

      <StatusBar response={response} indexStatus={indexStatus} t={t} />

      {menu && (
        <ContextMenu
          actions={menu.actions}
          position={menu.position}
          onAction={(action) => runAction(action, menu.result)}
          onClose={() => setMenu(null)}
          t={t}
        />
      )}

      {settingsOpen && (
        <SettingsPanel
          settings={settings}
          indexStatus={indexStatus}
          rebuilding={rebuilding}
          onChange={persistSettings}
          onRebuildIndex={rebuild}
          onResetUsage={() => {
            api
              .resetUsage()
              .then(() => showNotice(t("settings.resetUsage")))
              .catch((cause: unknown) => setActionError(asCommandError(cause)));
          }}
          onClose={() => setSettingsOpen(false)}
          t={t}
        />
      )}

      {confirmation && (
        <ConfirmDialog
          title={confirmation.title}
          body={confirmation.body}
          confirmLabel={confirmation.confirmLabel}
          destructive={confirmation.destructive}
          onConfirm={() => {
            const pending = confirmation;
            setConfirmation(null);
            pending
              .run()
              .catch((cause: unknown) => setActionError(asCommandError(cause)));
          }}
          onCancel={() => setConfirmation(null)}
          t={t}
        />
      )}

      {notice && (
        <div
          className="lce-menu fixed bottom-8 left-1/2 z-50 -translate-x-1/2 px-3 py-1.5 text-[12px]"
          role="status"
        >
          {notice}
        </div>
      )}
    </div>
  );
}