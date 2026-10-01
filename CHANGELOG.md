# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-10-01

The first runnable MVP. Everything below works today; nothing is a placeholder.

### Added

- **Rust workspace** with hard module boundaries: `search-core`, `query-dsl`,
  `ranking`, five `windows-*` providers, `search-daemon`, `search-cli` and
  `search-mcp`.
- **Unified entity model** (`LocalEntity`) with six variants — file, directory,
  process, application, service, window — and a single `SearchResult` shape.
- **Local Search DSL**: `type:`, `ext:`, `path:`, `name:`, `drive:`,
  `modified:`, `created:`, `size:`, `state:`, `pid:`, `user:`, `visible:`,
  `sort:` and `limit:`, with graceful fallback to plain text.
- **Rule-based natural language compiler** for English and Simplified Chinese.
  `最近的 PDF` and `recent PDFs` compile to the same query, as do
  `正在运行的 python` and `running python processes`.
- **Deterministic heuristic ranking** with named, configurable weights, fuzzy
  subsequence matching and a bounded local usage history.
- **Filesystem index** with two interchangeable backends: a directory scan that
  needs no privileges and records sizes, and an NTFS MFT + USN change journal
  backend that needs elevation but enumerates a volume in one pass and updates
  incrementally.
- **Compact index storage**: a string arena plus 48-byte records, so a one
  million entry index costs roughly 70 MB rather than several hundred.
- **Atomically persisted index cache** in `%LOCALAPPDATA%\LocalContextEngine`.
- **Live providers** for processes (Toolhelp32 + PSAPI, resolved in parallel),
  services (`EnumServicesStatusEx`), top-level windows (`EnumWindows`) and
  applications (App Paths, Start Menu, `PATH`, WindowsApps).
- **CLI** `localsearch.exe` with human and `--json` output, `--explain`,
  `--index-status`, `--rebuild-index`, `--update-index`, `--providers`,
  `--list-usage` and `--reset-usage`.
- **MCP server** `localsearch-mcp.exe` over stdio with eleven tools, including
  `search_system`, the five scoped search tools, `compile_query`,
  `index_status`, `dsl_reference`, `record_selection` and `terminate_process`.
- **Tauri 2 desktop shell** with a React + TypeScript + Tailwind interface:
  keyboard-first navigation, type filters, match highlighting, per-entity
  context menus, a confirmation dialog for the one destructive action, and
  full zh-CN/en-US localisation that switches without a restart.
- **290 tests** across the workspace plus workspace-level integration
  tests, benchmarks, and the `docs/` set below.

### Security

- File search is read-only.
- Process termination is gated behind an explicit confirmation on both the UI
  and the Rust side.
- No network access anywhere in the codebase.

[0.1.0]: https://github.com/styayur/local-context-engine/releases/tag/v0.1.0