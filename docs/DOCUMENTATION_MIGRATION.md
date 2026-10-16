# Documentation migration inventory

This inventory records documentation organization, not runtime changes.

## Canonical structure

- [ARCHITECTURE.md](ARCHITECTURE.md)
- [QUERY_ENGINE.md](QUERY_ENGINE.md)
- [INDEXING.md](INDEXING.md)
- [BENCHMARKS.md](BENCHMARKS.md)
- [SECURITY.md](SECURITY.md)
- [PROVIDERS.md](PROVIDERS.md)
- [DEVELOPMENT.md](DEVELOPMENT.md)
- [ROADMAP.md](ROADMAP.md)

## Moved files

| Previous path | Current path |
|---|---|
| `docs/UPGRADE-2026-10.md` | `docs/archive/upgrade-2026-10.md` |
| `docs/README_EVIDENCE.md` | `docs/archive/development-notes/readme-evidence-2026-10-08.md` |
| `docs/measurements/planner-after-build.txt` | `docs/archive/benchmark-history/planner-20261012/planner-after-build.txt` |
| `docs/measurements/planner-after-process.json` | `docs/archive/benchmark-history/planner-20261012/planner-after-process.json` |
| `docs/measurements/planner-after.csv` | `docs/archive/benchmark-history/planner-20261012/planner-after.csv` |
| `docs/measurements/planner-before-build.txt` | `docs/archive/benchmark-history/planner-20261012/planner-before-build.txt` |
| `docs/measurements/planner-before-process.json` | `docs/archive/benchmark-history/planner-20261012/planner-before-process.json` |
| `docs/measurements/planner-before.csv` | `docs/archive/benchmark-history/planner-20261012/planner-before.csv` |

## Added files and preserved snapshots

- `.markdownlint.json`
- `THIRD_PARTY_LICENSES.md`
- `docs/DEVELOPMENT.md`
- `docs/DOCUMENTATION_CHECKS.md`
- `docs/INDEXING.md`
- `docs/PROVIDERS.md`
- `docs/QUERY_ENGINE.md`
- `docs/README.md`
- `docs/RELEASE_PREPARATION.md`
- `docs/ROADMAP.md`
- `docs/SECURITY.md`
- `docs/archive/README.md`
- `docs/archive/benchmark-history/benchmarks-before-v1.md`
- `docs/archive/development-notes/architecture-before-v1.md`
- `docs/archive/development-notes/changelog-before-v1.md`
- `docs/archive/development-notes/readme-before-v1.md`
- `docs/archive/development-notes/readme-zh-before-v1.md`
- `docs/archive/development-notes/security-before-v1.md`
- `docs/archive/development-notes/upgrade-delivery-2026-10.md`
- `docs/releases/v1.0.0.md`
- `docs/DOCUMENTATION_MIGRATION.md` (this inventory).

## Rewritten entry points

README, CHANGELOG and canonical guides now describe user workflows and current
limits. Earlier README/architecture/security/changelog content remains in the
archive. Existing supporting references remain available. Relative links were
updated to follow moved files.

## Removed from current narrative

- PR-by-PR delivery logs and phase plans moved out of the user entry points.
- Unsupported performance comparisons and absolute safety/privacy claims were corrected.
- Obsolete safe-fix/version statements were replaced with current limitations.

No historical report, raw measurement, screenshot, core source, fixture or test
was intentionally deleted. File paths retired by moves are listed above.
Raw moved CSV/JSON/text captures were compared with their original Git contents
and retain identical content (allowing repository newline normalization).

## Verification scope

Markdown lint covers current guides and templates; archives retain historical
formatting. Link checks include archives and image targets. README GFM renders
were checked at desktop and narrow widths. Existing Rust tests/format/Clippy
were run without modifying tests or baseline. Original PR CI and Regression Gate
remain required. See [release preparation](RELEASE_PREPARATION.md) for blockers.
