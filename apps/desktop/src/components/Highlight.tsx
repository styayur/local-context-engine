import { Fragment, useMemo } from "react";

import type { MatchRange } from "../lib/types";

interface HighlightProps {
  text: string;
  ranges: MatchRange[];
  /**
   * Whether the ranges were computed against this exact string. The engine
   * computes them against the entity's `name`; a service's `display_name` can
   * differ, and highlighting the wrong characters is worse than highlighting
   * none.
   */
  applicable: boolean;
}

/**
 * Render `text` with the matched character ranges emphasised.
 *
 * Ranges are character offsets, not byte offsets, which is what keeps
 * highlighting correct for Chinese file names.
 */
export function Highlight({ text, ranges, applicable }: HighlightProps) {
  const segments = useMemo(() => {
    if (!applicable || ranges.length === 0) return null;
    const characters = Array.from(text);
    const parts: { text: string; marked: boolean }[] = [];
    let cursor = 0;
    for (const range of ranges) {
      const start = Math.max(0, Math.min(range.start, characters.length));
      const end = Math.max(start, Math.min(range.end, characters.length));
      if (start > cursor) {
        parts.push({ text: characters.slice(cursor, start).join(""), marked: false });
      }
      if (end > start) {
        parts.push({ text: characters.slice(start, end).join(""), marked: true });
      }
      cursor = Math.max(cursor, end);
    }
    if (cursor < characters.length) {
      parts.push({ text: characters.slice(cursor).join(""), marked: false });
    }
    return parts;
  }, [text, ranges, applicable]);

  if (!segments) return <>{text}</>;

  return (
    <>
      {segments.map((segment, index) => (
        <Fragment key={`${index}-${segment.marked ? "m" : "p"}`}>
          {segment.marked ? <span className="lce-mark">{segment.text}</span> : segment.text}
        </Fragment>
      ))}
    </>
  );
}