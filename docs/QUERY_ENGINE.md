# Query engine

## Query meaning

Explicit DSL filters, rule-based English/Chinese input and plain text compile
into SearchQuery. Recognized DSL takes precedence; unknown key-like text remains
search text. This is not general natural-language understanding. See the
[DSL reference](DSL.md) for supported syntax and units.

## Candidate planning

The file provider offers extension postings, a sorted prefix table and trigrams
over normalized full paths. The planner orders available sources by cardinality,
chooses a selective seed and intersects useful sources. A broad additional source
may be skipped when verifying the smaller seed costs less. Short tokens and
common trigrams do not discard a useful indexed condition in the same query.

The current candidate-planning budget is 250,000 IDs; a source more than eight
times the current seed can be skipped. These are implementation cost heuristics,
not DSL semantics or stable API promises. No selective source means linear
fallback. Missing trigrams can establish an empty candidate set; deliberately
pruned common trigrams cannot. Prefix keys retain 24 bytes; longer queries still
verify the full prefix. Accelerated IDs are sorted and deduplicated before the
same authoritative match/filter logic used by the reference path.

## Ranking and explanation

Ranking uses named heuristic components for name, prefix, tokens, path, fuzzy,
recency and optional usage. `--explain` exposes compilation and plan/ranking
information; `--providers` and `--index-status` expose provider health.

```powershell
localsearch --explain "ext:pdf report"
localsearch --explain "prefix:read"
localsearch --json "type:process python"
```

Different snapshots, clocks, usage history or fuzzy settings can change results.
Stable ordering applies to identical inputs. Broad/common-only or fuzzy queries
may still scan; see [Benchmarks](BENCHMARKS.md). Completeness beyond a requested
result/candidate budget is not promised.

## Correctness checks

Tests compare accelerated and linear results, including order and caps; they
cover absent/common trigrams, combined conditions, Unicode and long prefixes.
Mutation/persistence tests protect accelerator consistency. Performance changes
must preserve these checks; an incomplete index can lose matches and is not safe
merely because candidates are later verified.
