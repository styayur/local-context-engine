<div align="right">
  <a href="./README.zh-CN.md">简体中文</a> | <strong>English</strong>
</div>

<div align="center">

<img src="docs/assets/brand/app-icon.png" width="96" alt="Local Context Engine" />

# Local Context Engine

**Millisecond, offline-first search over your own machine.**

Natural language in → deterministic query → Rust search core → results.

[Download](https://github.com/styayur/local-context-engine/releases/latest) · [Architecture](docs/ARCHITECTURE.md) · [DSL](docs/DSL.md) · [MCP](docs/MCP.md) · [Benchmarks](docs/BENCHMARKS.md)

[![CI](https://github.com/styayur/local-context-engine/actions/workflows/ci.yml/badge.svg)](https://github.com/styayur/local-context-engine/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/styayur/local-context-engine?label=release&color=orange)](https://github.com/styayur/local-context-engine/releases)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-black?logo=rust&logoColor=white)]()
[![Tauri](https://img.shields.io/badge/Tauri-2-24C8D8?logo=tauri&logoColor=white)]()
[![Windows](https://img.shields.io/badge/Windows-10%20%2F%2011-0078D6?logo=windows&logoColor=white)]()
[![offline](https://img.shields.io/badge/offline--first-0f172a)]()
[![no telemetry](https://img.shields.io/badge/telemetry-none-0f172a)]()

</div>

---

What is this?
--------------

Local Context Engine is a local search tool for Windows: one input box that
finds your **files and folders**, **applications**, **running processes**,
**services** and **windows** at the same time, and opens them.

Type `vscode` and you get the application, the running process and any matching
file in one list. Type `正在运行的 python` or `running python processes` and you
get the processes. Type `最近的 pdf` or `recent PDFs` and you get today's PDFs,
newest first.

It is built from three ideas:

1. **Search must be deterministic.** The same query returns the same list in the
   same order, every time. No model, no embeddings, no vector database sits on
   the hot path.
2. **Understanding is a compiler problem.** Natural language is *compiled* into
   a structured query — `最近的 pdf` becomes `type:file ext:pdf
   sort:modified-desc` — and then a Rust engine answers it.
3. **Offline is the default, not a mode.** There is no account, no telemetry and
   no network code in the repository.

Why it exists
-------------

Desktop search on Windows is either a full-text indexer that rebuilds for
minutes, or a launcher that cannot see anything except shortcuts. Meanwhile the
"AI" answer to search is usually "embed everything and hope" — which is slow,
non-deterministic, unauditable, and quietly sends your file names somewhere.

This project takes the opposite position:

> AI, if it appears at all, is a **query compiler**. Retrieval is a
> deterministic local operation.

How it differs from a traditional desktop search
------------------------------------------------

|                                   | Traditional indexer | Launcher   | Local Context Engine                          |
|-----------------------------------|---------------------|------------|-----------------------------------------------|
| Time to first useful result       | minutes of indexing | instant    | instant for live data, one scan for files      |
| Indexes file **contents**         | yes                 | no         | no — names, paths and metadata only            |
| Sees processes/services/windows   | rarely              | no         | yes, all four                                  |
| Query language                    | one text box        | one text box | text box **and** a documented DSL **and** MCP |
| Deterministic ranking             | usually             | usually    | always, with `--explain` to show the arithmetic |
| AI on the search path             | sometimes           | sometimes  | never                                          |
| Network required                  | often               | no         | never                                          |
| Memory footprint                  | hundreds of MB      | tens of MB | ~70 MB for a one million entry index           |

Architecture
------------

```text
             User / AI
                 │
                 ▼
          Query Compiler            ← the only place an AI could ever plug in
                 │
                 ▼
           SearchQuery
                 │
                 ▼
           Search Core
                 │
     ┌───────────┼───────────┐
     ▼           ▼           ▼
   Files      Processes     Apps
     │           │           │
  MFT/USN       Win32      Registry
```

The workspace is split so that "where does this belong?" never needs a meeting:

```text
local-context-engine/
├─ apps/desktop/            Tauri 2 + React + TypeScript shell
├─ crates/
│  ├─ search-core/          entity/query/result model, engine, fuzzy matcher
│  ├─ query-dsl/            the DSL parser and the rule-based NL compiler
│  ├─ ranking/              heuristic scoring + bounded local usage memory
│  ├─ windows-files/        directory-scan and NTFS MFT + USN index backends
│  ├─ windows-processes/    live process snapshots
│  ├─ windows-apps/         App Paths, Start Menu, PATH, WindowsApps
│  ├─ windows-services/     read-only service snapshots
│  ├─ windows-windows/      top-level window snapshots
│  ├─ search-daemon/        the composition root every front end shares
│  ├─ search-cli/           localsearch.exe
│  ├─ search-mcp/           localsearch-mcp.exe (MCP over stdio)
│  └─ benchmarks/           Criterion benchmarks for 10k / 100k / 1M entries
├─ docs/
└─ tests/                   workspace-level integration tests
```

Full detail — including why `AI != Search Engine`, the provider boundary and the
memory arithmetic — is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

Local Search DSL
----------------

```text
chrome
type:file rust
type:file ext:rs
type:file ext:pdf modified:<24h
type:process python
type:process name:node
type:app vscode
type:service state:running
type:window github
path:projects ext:toml
drive:C ext:exe sort:size-desc
path:"My Projects" ext:toml
size:>10mb sort:size-desc limit:20
```

Keys: `type`, `ext`, `path`, `name`, `drive`, `modified`, `created`, `size`,
`state`, `pid`, `user`, `visible`, `sort`, `limit`. Parsing never fails — an
unrecognised key stays free text, so `localsearch "C:\Users\me"` is a search
rather than a syntax error.

The full reference, including the time and size operators, is in
[docs/DSL.md](docs/DSL.md).

MCP
---

`localsearch-mcp.exe` exposes the same core to any MCP client over stdio:

```jsonc
{
  "mcpServers": {
    "localsearch": { "command": "C:\\path\\to\\localsearch-mcp.exe" }
  }
}
```

| tool                | what it does                                     |
|---------------------|--------------------------------------------------|
| `search_system`     | search every domain at once                      |
| `search_files`      | files and folders                                |
| `search_processes`  | running processes                                |
| `search_apps`       | installed applications                           |
| `search_services`   | Windows services                                 |
| `search_windows`    | open top-level windows                           |
| `compile_query`     | show how an input compiles, without searching    |
| `index_status`      | backend, entry counts, memory, provider health   |
| `dsl_reference`     | the key table and examples                       |
| `record_selection`  | boost a result next time                         |
| `terminate_process` | destructive; requires `confirm: true`            |

See [docs/MCP.md](docs/MCP.md).

CLI
---

```bash
localsearch chrome
localsearch "type:file ext:rs"
localsearch "正在运行的 python"
localsearch --type process node
localsearch --json vscode
localsearch --explain "昨天修改的 rust 文件"
localsearch --index-status
localsearch --rebuild-index
localsearch --update-index
localsearch --providers
localsearch --list-usage
localsearch --reset-usage
```

Exit codes are part of the contract: `0` results found, `1` valid query with no
matches, `2` the command could not be completed. `--json` writes a single JSON
document to stdout; diagnostics always go to stderr.

Desktop
-------

```bash
cd apps/desktop
npm install
npm run tauri:dev      # development, hot reload
npm run tauri:build    # NSIS installer in src-tauri/target/release/bundle
```

* Autofocused search box, keyboard-first: `↑`/`↓` navigate, `Enter` opens,
  `Shift+Enter` opens the context menu, `Tab` cycles the type filters, `Esc`
  clears or closes, `Ctrl+,` opens settings.
* Type filter chips with live counts, match highlighting, per-entity context
  menus, and a status bar showing the compile result and the elapsed time.
* **Simplified Chinese and English**, switchable without a restart. Every
  string — errors, empty states, tooltips, context menus — lives in
  `src/i18n/{zh-CN,en-US}.json`; nothing is hard-coded in a component.
* Settings for language, theme, result limit, fuzzy matching, usage ranking,
  indexed drives, index backend, the global shortcut, and a full index panel.
* The only destructive action, ending a process, is behind a confirmation
  dialog whose default button is Cancel — and the Rust side refuses the call
  without an explicit `confirmed: true`.

Build
-----

Requirements: Rust (stable, MSVC toolchain), Visual Studio Build Tools with
"Desktop development with C++", Node 20+, and WebView2 (already present on
Windows 11).

```bash
git clone https://github.com/styayur/local-context-engine
cd local-context-engine

# CLI + MCP server
cargo build --release

# Desktop shell + installer
cd apps/desktop
npm install
npm run tauri:build
```

The binaries land in `target/release/`:

```text
target/release/localsearch.exe
target/release/localsearch-mcp.exe
```

Development
-----------

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test

cd apps/desktop
npm run lint
npm run typecheck
npm run build
```

Benchmarks:

```bash
cargo bench -p lce-benchmarks --bench file_search
cargo bench -p lce-benchmarks --bench ranking
```

Layout, conventions and how to add a provider or a translation are covered in
[CONTRIBUTING.md](CONTRIBUTING.md).

Security
--------

* File search is **read-only**.
* No `exec`, no `eval`, no shell string built from a query.
* Persisted state is a MessagePack index and two JSON files this project wrote;
  no untrusted payload is ever deserialised as code.
* Paths are treated as opaque Unicode strings and are tested with spaces,
  Simplified Chinese and very long names. Junctions and symlinks are never
  followed.
* Process termination requires confirmation on both sides of the boundary.
* Services are strictly read-only in this version.

Full threat model: [SECURITY.md](SECURITY.md).

Privacy
-------

> **Your local index never leaves your machine.**

* No telemetry, no analytics, no crash reporting, no update check.
* No account, no login, no cloud service.
* Query history is optional, bounded, stored locally in
  `%LOCALAPPDATA%\LocalContextEngine\usage.json`, and removable from Settings or
  with `localsearch --reset-usage`.
* There is no HTTP client in the dependency tree of the search core.

Benchmarks
----------

Measured with Criterion on a synthetic corpus, not estimated. Full table,
machine specification, method and caveats: [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

| what | 1 000 000 indexed entries |
|---|---:|
| query compilation (`最近的 pdf`, `recent PDFs`, plain text, DSL) | 1.4 – 11.6 µs |
| file search that fills the candidate budget (the common typing case) | **~8 ms** |
| file search that must visit every record | **~62 ms** |
| the same, with fuzzy matching on every record | **~76 ms** |
| ranking 4 000 candidates | **~2.8 ms** |
| full hot path: walk the index, collect, rank | **~10 ms** |
| building the in-memory index | 316 ms (3.2 M entries/s) |
| memory for one million entries | ≈ 69 MB |

The candidate budget (`CANDIDATE_CAP = 4 000`) is why a one million entry index
does not cost fifty times a ten thousand entry one. The rows that do grow are
the ones that genuinely have to touch every record.

Roadmap
-------

* MSIX/Appx discovery through `AppModel\Repository` rather than Start Menu links.
* File sizes from the MFT by parsing the `$DATA` attribute.
* Event-driven index updates instead of replaying the journal on demand.
* An optional frameless palette window.
* An optional AI query compiler behind the existing `QueryCompiler` trait —
  off by default, and never on the search path.

Licence
-------

[MIT](LICENSE). Not affiliated with, derived from, or containing any code,
assets or UI of Listary or any other launcher. The only thing borrowed is the
public idea that a local index should be fast.