# Local Context Engine

Offline Windows search and local stdio MCP for files, applications, processes, services and windows.

[简体中文](README.zh-CN.md) · [Download v0.2.0](https://github.com/styayur/local-context-engine/releases/tag/v0.2.0) · [Documentation](docs/README.md)

**Stage: Alpha.** Implemented workflows have regression coverage; independent installation, upgrade and sustained user validation remain outstanding. This guide describes current source. The latest published release is v0.2.0; the v1.0.0 notes are an unreleased candidate, not an available binary or compatibility guarantee.

Current Rust workspace, desktop package and installer configuration declare 0.2.0. Sharing a version number with the release does not mean every default-branch change is in its binaries.

## Features

- Search local filenames, paths and metadata with an explicit DSL or English/Chinese rule-based query compilation. No account, model service or network is needed for normal search.
- Query files/directories, apps, processes, services and windows through the same service. Inspect ranking explanations, provider health and index state.
- Use the CLI, desktop shell or local stdio MCP adapter. The optional elevated NTFS MFT/USN service is separate from the unprivileged scan backend.
- File-query accelerators narrow candidates before exact verification; broad queries can still scan. Deterministic ordering requires the same query, snapshot, settings and usage state.

```text
CLI / Desktop / MCP → SearchService → Providers
                                      ├─ file: planner → accelerators → verification
                                      └─ apps / processes / services / windows
                                  → ranking and explanations
```

The planner belongs to the file provider, not to every provider. See [Architecture](docs/ARCHITECTURE.md) and [Query Engine](docs/QUERY_ENGINE.md). The existing [benchmark report](docs/BENCHMARKS.md) records synthetic measurements and their limits; it is not a latency guarantee.

## Installation / Quick Start

### Published Windows packages

Download the [Windows x64 portable ZIP](https://github.com/styayur/local-context-engine/releases/download/v0.2.0/local-context-engine-windows-x64-v0.2.0.zip) or [desktop installer](https://github.com/styayur/local-context-engine/releases/download/v0.2.0/local-context-engine-desktop-v0.2.0-setup.exe). Compare your file with [SHA256SUMS.txt](https://github.com/styayur/local-context-engine/releases/download/v0.2.0/SHA256SUMS.txt):

```powershell
Get-FileHash <downloaded-file> -Algorithm SHA256
```

Before the first file search, review [index scope](docs/INDEXING.md): an empty `indexedDrives` setting selects all fixed local volumes, up to the entry limit. A `path:` filter restricts results, not collection. With clients closed, edit `%LOCALAPPDATA%\LocalContextEngine\settings.json` to select drives. If it does not exist, create that directory and a UTF-8 JSON file; this example selects only C. For an existing file, change only `indexedDrives` and preserve other settings:

```json
{"indexedDrives": ["C"]}
```

Use valid JSON; an unreadable or invalid settings file falls back to defaults. Restart clients, and rebuild an existing index after changing scope. Back up settings and optional `%LOCALAPPDATA%\LocalContextEngine\usage.json` to preserve preferences; the index is rebuildable local metadata.

Extract the portable archive. In PowerShell, open the directory containing `localsearch.exe` and run:

```powershell
.\localsearch.exe --help
.\localsearch.exe --providers
.\localsearch.exe --backend scan "type:file ext:pdf"
.\localsearch.exe --explain "ext:pdf report"
.\localsearch.exe "type:process python"
.\localsearch.exe --index-status
```

The scan backend does not require the optional privileged service. Search returns `0` for results, `1` for a valid query with no matches, and `2` for an operation that could not complete. Provider availability depends on Windows permissions; live results and relative-time filters change with the system. See the [DSL reference](docs/DSL.md) and [indexing guide](docs/INDEXING.md).

For the desktop package, run the installer, then launch **Local Context Engine**. It requires WebView2 and acceptance on your actual Windows installation. Build success alone does not establish that its window renders and searches correctly.

### Build current source

Use Windows x64, a current stable Rust MSVC toolchain and Visual Studio C++ Build Tools. For the desktop, also use Node.js 22 (CI), npm and WebView2.

```powershell
git clone https://github.com/styayur/local-context-engine.git
cd local-context-engine
cargo build --release --locked
.\target\release\localsearch.exe --help
```

These commands build the current default branch, not the published v0.2.0 tag. Default Cargo members include the CLI and MCP server but exclude the desktop stack. Desktop packaging and output paths are documented in [Development](docs/DEVELOPMENT.md).

To launch the desktop from source, run `npm ci` then `npm run tauri:dev` in `apps/desktop`; Tauri opens the native window and starts its development server.

### Connect an MCP client

After building, `target/release/localsearch-mcp.exe` serves MCP over local stdio. Replace the example path with its absolute path in your client's configuration:

```json
{
  "mcpServers": {
    "localsearch": {
      "command": "C:\\tools\\lce\\localsearch-mcp.exe"
    }
  }
}
```

See [MCP tools and protocol examples](docs/MCP.md) for `search_system`, `compile_query` and other tools. Search itself is local; your MCP client or model may forward results to another service, under its own privacy settings.

## Testing / CI / Platform Support

[![CI](https://github.com/styayur/local-context-engine/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/styayur/local-context-engine/actions/workflows/ci.yml)

From the repository root on Windows:

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release --locked
```

For desktop checks, run in `apps/desktop`:

```sh
npm ci
npm run lint
npm run typecheck
npm run build
```

The [CI workflow](.github/workflows/ci.yml) runs Rust and desktop checks on Windows, including `cargo check -p local-context-engine-desktop` from the repository root. [CodeForge regression](.github/workflows/codeforge-regression.yml) is an external development gate, not a runtime or Rust crate dependency. See [documentation checks](docs/DOCUMENTATION_CHECKS.md) for Markdown and link validation.

## Known Limitations

- Windows is the supported runtime backend; Linux runtime support is not provided. Cargo declares Rust 1.82, but CI tests current stable rather than that minimum.
- File search indexes names, paths and metadata, not file contents. Full-text search, semantic retrieval and model-backed query compilation are future work, not current capabilities.
- The elevated MFT/USN service and desktop need validation across machines. Earlier blank-WebView behavior is not resolved by a successful build alone. Service ACLs, journal rollover, fallback and uninstall need deployment acceptance.
- Cached paths, metadata, settings and optional usage history are sensitive local data. The elevated service can reveal filenames that a normal directory walk cannot list; it does not grant file-content access. Read [Security](docs/SECURITY.md) before enabling it on a shared machine.

## Roadmap

Before publishing v1.0, update package versions together, validate the Rust minimum, and complete fresh-install, desktop-rendering and service acceptance gates. Metadata completeness and application discovery can improve later; semantic/full-text extensions must be explicit opt-in. See [Roadmap](docs/ROADMAP.md) and [release preparation](docs/RELEASE_PREPARATION.md), which distinguish implemented workflows from experimental deployment paths and future work.

## Contributing

Follow [CONTRIBUTING.md](CONTRIBUTING.md) and [Development](docs/DEVELOPMENT.md). Use [Issues](https://github.com/styayur/local-context-engine/issues) for reproducible bugs and [SECURITY.md](SECURITY.md) for vulnerabilities. Keep permission boundaries and English/Chinese capability statements aligned.

## License

The repository [LICENSE](LICENSE) and Rust workspace declare **MPL-2.0**. Desktop package metadata still says MIT; [third-party license notes](THIRD_PARTY_LICENSES.md) record that unresolved inconsistency. This documentation change does not alter licenses or package manifests. Dependencies retain their own terms; release notice completeness remains a separate review item.
