/**
 * These mirror the Rust DTOs one for one. If a field changes in
 * `crates/search-core`, it changes here in the same commit.
 */

export type EntityType =
  | "file"
  | "directory"
  | "process"
  | "application"
  | "service"
  | "window";

export interface MatchRange {
  start: number;
  end: number;
}

export function matchRangeEquals(a: MatchRange, b: MatchRange): boolean {
  return a.start === b.start && a.end === b.end;
}

export interface SearchResult {
  id: string;
  entity_type: EntityType;
  name: string;
  display_name: string;
  path: string | null;
  subtitle: string;
  score: number;
  metadata: Record<string, string>;
  match_ranges: MatchRange[];
  matched_field: "name" | "path" | "none";
  entity: LocalEntity;
}

export type LocalEntity =
  | { kind: "file"; [key: string]: unknown }
  | { kind: "directory"; [key: string]: unknown }
  | { kind: "process"; [key: string]: unknown }
  | { kind: "application"; [key: string]: unknown }
  | { kind: "service"; [key: string]: unknown }
  | { kind: "window"; [key: string]: unknown };

export interface ProviderTiming {
  provider: string;
  candidates: number;
  elapsed_ms: number;
}

export interface SearchResponse {
  query: string;
  compiled: string;
  elapsed_ms: number;
  total: number;
  truncated: boolean;
  results: SearchResult[];
  timings: ProviderTiming[];
  warnings: string[];
}

export type Language = "system" | "zh-CN" | "en-US";
export type Theme = "system" | "light" | "dark";
export type IndexBackend = "auto" | "scan" | "mft-usn";

export interface Settings {
  language: Language;
  theme: Theme;
  resultLimit: number;
  fuzzyMatching: boolean;
  usageRanking: boolean;
  indexBackend: IndexBackend;
  indexedDrives: string[];
  maxIndexEntries: number;
  includeDirectories: boolean;
  hotkey: string;
  includeHiddenWindows: boolean;
}

export interface ProviderStats {
  name: string;
  scope: "live" | "cached" | "hybrid";
  entity_types: EntityType[];
  entity_count: number | null;
  ready: boolean;
  detail: Record<string, string>;
  warnings: string[];
}

export interface IndexReport {
  backend: string;
  volumes: string[];
  entries: number;
  directories: number;
  truncated: boolean;
  elapsedMs: number;
  warnings: string[];
}

export interface IndexStatus {
  ready: boolean;
  backend: string;
  requestedBackend: string;
  entries: number;
  files: number;
  directories: number;
  memoryBytes: number;
  volumes: string[];
  cachePath: string;
  cacheAgeMs: number | null;
  lastReport: IndexReport | null;
  providers: ProviderStats[];
}

export interface CommandError {
  code: string;
  message: string;
}

/** Map a thrown value onto the structured error the UI renders. */
export function asCommandError(error: unknown): CommandError {
  if (typeof error === "object" && error !== null && "code" in error) {
    const candidate = error as { code?: unknown; message?: unknown };
    return {
      code: String(candidate.code ?? "unknown"),
      message: String(candidate.message ?? ""),
    };
  }
  if (error instanceof Error) {
    // Tauri rejects with a stringified `LceError`; keep the raw text for the
    // developer but let the UI translate the code.
    const code = /code: ([a-z-]+)/.exec(error.message)?.[1];
    return { code: code ?? "unknown", message: error.message };
  }
  return { code: "unknown", message: String(error) };
}

export const FILTER_TYPES: { key: "all" | EntityType; labelKey: string }[] = [
  { key: "all", labelKey: "search.all" },
  { key: "file", labelKey: "search.files" },
  { key: "directory", labelKey: "search.folders" },
  { key: "application", labelKey: "search.apps" },
  { key: "process", labelKey: "search.processes" },
  { key: "service", labelKey: "search.services" },
  { key: "window", labelKey: "search.windows" },
];