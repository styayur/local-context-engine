# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-10-01

A v0.2 engineering pass over the v0.1 MVP. Nothing that worked in 0.1 was
removed: the CLI, the MCP server, the desktop shell, the bilingual UI, the Local
Search DSL, the ranking crate and the `SearchService` API all keep their
behaviour.

### Added

- **Per-volume journal state** (`windows_files::journal`). `JournalRegistry`
  keys every NTFS volume by a stable `VolumeId` — a volume GUID, never a drive
  letter — and tracks its own journal id, cursor, first/next USN and sync
  status. A journal that is recreated, that rolls over, or that moves backwards
  marks **only that volume** stale and schedules a rebuild.
- **Event-driven index workers** (`windows_files::worker`). One `VolumeWorker`
  per volume tails its own USN journal with a `Timeout`/`BytesToWaitFor` read,
  so the thread blocks in the kernel instead of polling. An `IndexSupervisor`
  owns the threads, tracks per-volume lifecycle (`starting`, `running`,
  `backing-off`, `failed`, `stopped`) and backs off exponentially on failure.
- **A privileged index service** (`lce-index-service.exe`) plus
  `index-protocol` and `index-client`. The desktop, CLI and MCP server stay
  unprivileged; the service owns the MFT and the journals and answers typed,
  versioned requests over a Named Pipe whose DACL is built from an explicit SDDL
  string (SYSTEM, Administrators and the installing user — never Everyone or
  Anonymous). `FILE_FLAG_FIRST_PIPE_INSTANCE` defeats name squatting and
  `PIPE_REJECT_REMOTE_CLIENTS` refuses remote callers at the pipe itself.
  `install`, `uninstall`, `start`, `stop`, `status` and `--console` are
  supported.
- **Search accelerators** (`windows_files::accelerators`): an extension index, a
  prefix table and a trigram index over normalised **full paths**, with
  high-frequency trigrams dropped and a compaction policy driven by mutation
  drift.
- **A query planner** (`windows_files::planner`) that picks
  `extension` / `prefix` / `trigram` / `intersection` / `linear` per query and
  reports what it did. `localsearch --explain` prints the plan, the candidate
  counts and the elapsed time.
- **A unified mutation model** (`IndexMutation`) so USN parsing and index
  application are separate steps, applied as one batch under a short write lock.
- **`prefix:` in the DSL**: the one predicate a prefix table can answer exactly.
- **Differential tests**: accelerated search must equal the pre-v0.2 linear scan
  over a randomised corpus, after mutations, and when the candidate cap
  truncates. This is what caught two real bugs during development (trigrams
  indexed names but not paths; a live insert could resurrect a dropped trigram
  and silently lose matches).

### Changed

- The persisted index schema is now **version 3** and carries the per-volume
  journal registry plus the accelerators. A v0.1 or v0.2 cache is discarded and
  rebuilt rather than migrated — guessing at an older cursor is how a stale
  index happens.
- MessagePack is now written with **named** fields. Positional encoding plus
  `skip_serializing_if` shifted every field after a skipped `Option` and
  corrupted the read.
- The file provider holds its store and accelerators behind one `RwLock`, and
  applies journal batches under that lock only after all disk I/O and parsing is
  done. Cache persistence happens outside the lock.
- Process detail resolution is chunked across up to eight threads.

### Fixed

- A single `Option<JournalState>` could not describe a machine with more than
  one NTFS volume; one disk's cursor could overwrite another's.

### Security

- Threat model documented for the privileged boundary, the pipe ACL, malformed
  clients, protocol downgrade, stale indexes and service crashes. It states
  plainly that MFT enumeration can reveal file *names* the calling user could
  not otherwise list, and why that is inherent to the technique.
- The protocol has no `Execute`, `Shell` or `Command` message, and a test
  enumerates the variants so adding one is a visible diff.

### Known limitations

- The accelerators roughly **double** the index footprint (measured: +60% at
  100 000 entries), which pushes a one million entry index past the project's
  100 MB goal. The prefix table is the largest single contributor.
- Common-only queries (`readme`, `report`) and fuzzy queries still fall back to
  a linear scan, because their trigrams are too common to narrow anything.
  A `path:` filter also forces a scan. These are unchanged from v0.1 in cost.
- A `Changes` subscription reports volume-level journal advances, not a
  per-file event stream.
- The MFT backend still cannot report file sizes.

[0.2.0]: https://github.com/styayur/local-context-engine/releases/tag/v0.2.0
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