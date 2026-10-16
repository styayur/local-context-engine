# Architecture

LCE separates query meaning, operating-system access, candidate planning and
ranking. The same SearchService composes these parts for CLI, Desktop and MCP.

## Data flow

```text
Input → query-dsl → SearchQuery → SearchService / Search Core
                                    ├─ file provider → planner → accelerators
                                    │                    ↓          ↓
                                    │              authoritative candidate verification
                                    └─ app / process / service / window snapshots
                                                       ↓
                                      ranking → stable tie-breaking → results
```

A candidate index is a conservative filter, not the definition of a match.
Unindexable conjuncts remain in verification. The same query, snapshot, settings
and usage state produce the same ordered results; live state and relative time
are explicit inputs. Result and candidate budgets bound work, so a capped result
is not a claim that every matching record was returned.

## Responsibilities

| Module | Owns |
|---|---|
| `search-core` | Entity, query, result, matcher and provider/ranker interfaces |
| `query-dsl` | Explicit DSL and rule-based English/Chinese compilation |
| `ranking` | Named score components and local usage weighting |
| `windows-files` | Store, planner, accelerators, scan/MFT/USN, persistence |
| Other `windows-*` providers | Domain-specific Windows snapshots |
| `search-daemon` | Composition, settings and user actions; not a required background daemon |
| `index-protocol`, `index-client`, `index-service` | Optional privileged process boundary |
| CLI, MCP and Desktop | Presentation and transport |

See [Query Engine](QUERY_ENGINE.md), [Indexing](INDEXING.md) and
[Providers](PROVIDERS.md) for the implemented interfaces and constraints.

## Reliability boundaries

Per-volume identity and journal cursors prevent one volume's failure from
invalidating another. Store and accelerators mutate together under a write lock;
journal I/O and parsing occur outside that critical section. Persisted state is
versioned, and unsupported caches are rebuilt. Health/degraded states are visible
to callers rather than being represented as proof of freshness.

The optional index service answers typed requests over a local named pipe.
Search is read-only; activation and confirmed process termination remain in the
user process. [Security](SECURITY.md) documents what that boundary does and does
not protect. Full-text or semantic providers are [future work](ROADMAP.md), not
part of the current search path.
