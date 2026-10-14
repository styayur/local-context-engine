# Changelog

User-visible release history. v1.0.0 is a preparation target, not a published
version. Earlier detailed engineering notes remain in the documentation archive.

## v1.0.0

**Unreleased draft.** This section consolidates the intended first stable release
scope, including capabilities already shipped in v0.2.0; it does not imply that
they were all introduced after that release.

### Added

- User guides for query planning, indexing, providers, security and development.
- A release-oriented capability map that distinguishes Stable, Experimental and Future.
- Reproducible million-record performance evidence with complete query results and limitations.

### Changed

- Installation and usage documentation now leads with local Windows discovery and explainable queries.
- Development history and raw measurement captures are preserved under `docs/archive`.
- Compatibility statements describe the implemented CLI/query workflows without promising universal desktop or service deployment support.

### Performance

- Existing selective planning retains useful indexed conditions when other terms require verification.
- Existing early posting pruning reduces peak construction working set in the recorded million-record probe; retained accelerator bytes are unchanged.
- Common-query fallback and workload/hardware limits remain explicit; this documentation revision introduces no runtime performance change.

### Security

- Clarified offline metadata indexing, local cache sensitivity and the optional service's MFT filename-visibility boundary.
- Corrected README license identification to MPL-2.0, matching LICENSE and Cargo metadata.
- No changes to permissions, protocols, persistence, application logic or tests.

## v0.2.0 — 2026-10-13

Published Windows release with extension/prefix/trigram accelerators,
cost-aware candidate planning, `prefix:` queries, per-volume MFT/USN state,
optional privileged indexing service and differential/index consistency tests.
Long-prefix and combined-filter correctness is verified against linear search.
Peak construction memory is lower in the paired probe; broad queries can still
scan. Existing schema-3 caches and explicit service permissions remain the
documented boundaries. See [benchmarks](docs/BENCHMARKS.md) and
[indexing](docs/INDEXING.md) for limitations.

## v0.1.0 — 2026-10-01

Initial Windows preview: local query DSL, bilingual rule-based compilation,
deterministic ranking, file/application/process/service/window providers,
CLI, stdio MCP and desktop shell. Optional usage history and explicit process
termination confirmation were included.

Detailed historical notes are retained in the
[original changelog](docs/archive/development-notes/changelog-before-v1.md).
