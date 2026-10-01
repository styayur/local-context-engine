import { useCallback, useEffect, useRef, useState } from "react";

import * as api from "../lib/tauri";
import { asCommandError, type CommandError, type SearchResponse } from "../lib/types";

export type SearchState = "idle" | "searching" | "ready" | "error";

export interface UseSearchResult {
  state: SearchState;
  response: SearchResponse | null;
  error: CommandError | null;
  /** True while the index is being built in a background command. */
  rebuilding: boolean;
  rebuild: () => Promise<void>;
  clearError: () => void;
}

/**
 * Debounced search.
 *
 * The debounce is deliberately short (60 ms): the Rust core answers a warm
 * query in single-digit milliseconds, so waiting longer would only make the
 * interface feel slow. Queries are also dropped when a newer one supersedes
 * them, so a fast typist can never see an out-of-order result set.
 */
export function useSearch(
  query: string,
  types: string[],
  limit: number,
  enabled: boolean,
  onIndexStatusChanged?: () => void,
): UseSearchResult {
  const [state, setState] = useState<SearchState>("idle");
  const [response, setResponse] = useState<SearchResponse | null>(null);
  const [error, setError] = useState<CommandError | null>(null);
  const [rebuilding, setRebuilding] = useState(false);
  const requestId = useRef(0);

  useEffect(() => {
    if (!enabled) return;
    const trimmed = query.trim();
    if (trimmed.length === 0) {
      requestId.current += 1;
      setState("idle");
      setResponse(null);
      setError(null);
      return;
    }

    const current = requestId.current + 1;
    requestId.current = current;
    setState("searching");

    const handle = window.setTimeout(() => {
      api
        .search(trimmed, types, limit)
        .then((result) => {
          if (requestId.current !== current) return;
          setResponse(result);
          setError(null);
          setState("ready");
        })
        .catch((cause: unknown) => {
          if (requestId.current !== current) return;
          setError(asCommandError(cause));
          setState("error");
        });
    }, 60);

    return () => window.clearTimeout(handle);
  }, [query, types, limit, enabled]);

  const rebuild = useCallback(async () => {
    setRebuilding(true);
    try {
      await api.rebuildIndex();
      setError(null);
      onIndexStatusChanged?.();
    } catch (cause: unknown) {
      setError(asCommandError(cause));
      setState("error");
    } finally {
      setRebuilding(false);
    }
  }, [onIndexStatusChanged]);

  const clearError = useCallback(() => setError(null), []);

  return { state, response, error, rebuilding, rebuild, clearError };
}