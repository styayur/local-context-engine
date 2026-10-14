# Local Context Engine

**Local-first context discovery engine for Windows.** Find files, applications,
processes, services and windows through one explainable local query.

[![CI](https://github.com/styayur/local-context-engine/actions/workflows/ci.yml/badge.svg)](https://github.com/styayur/local-context-engine/actions/workflows/ci.yml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue)](LICENSE)

[简体中文](README.zh-CN.md) · [Download v0.2.0](https://github.com/styayur/local-context-engine/releases/tag/v0.2.0) · [Documentation](docs/README.md)

Current published release: **v0.2.0**. The v1.0.0 notes are an **unreleased
candidate**, not an available binary or a new compatibility guarantee.

## Why Local Context Engine

A filename, an installed application and a running process can refer to the same
work. LCE puts those local sources behind one query interface, while keeping the
query, candidate selection and ranking inspectable. File search indexes names,
paths and metadata rather than file contents. Normal search needs no account,
model service or network connection.

## Features and maturity

| Status | Capability | Boundary |
|---|---|---|
| Stable | Query DSL and English/Chinese rule-based query compilation | Explicit filters; no model-generated retrieval |
| Stable | Query Planner with extension, prefix and full-path trigram accelerators | Exact verification remains authoritative; broad queries can scan |
| Stable | File/directory, app, process, service and window providers | Availability and metadata depend on Windows permissions |
| Stable | CLI, local stdio MCP, ranking explanations and index health | Same query **and snapshot/settings/usage state** give deterministic ordering |
| Experimental | Optional privileged NTFS MFT/USN service and desktop acceptance across machines | Implemented and tested, but require deployment-specific validation; see security and roadmap |
| Future | Full-text, semantic retrieval and model-backed query compilation | Not installed, enabled or run by default; no current implementation promise |

Stable means implemented workflows with regression coverage. It does not mean
all hardware, filesystems or desktop environments have been certified.

## Architecture

```text
User
  ↓
CLI / Desktop / MCP
  ↓
Search Core (via SearchService)
  ↓
Providers ── live Windows snapshots
  ↓ file-provider branch
Query Planner → Accelerators → candidate verification
  ↓
Ranking + deterministic ordering + explanations
```

The planner lives inside the file provider. Accelerators narrow candidates;
providers own operating-system access. The interfaces, data flow and failure
boundaries are described in [Architecture](docs/ARCHITECTURE.md).

## Performance

A paired synthetic **one-million-record** probe measured the following on
Windows with Rust 1.98.1. These are local observations, not latency guarantees.

| Measurement | Before planner optimization | After |
|---|---:|---:|
| Accelerator construction | 11.93 s | 8.91 s |
| Construction process peak working set | 1005.25 MB | 258.98 MB |
| `ext:pdf report`, mean / P95 | 89.36 / 99.74 ms | 20.95 / 23.14 ms |
| Common `report`, mean / P95 | 8.34 / 9.61 ms | 8.47 / 9.87 ms |
| Estimated retained accelerator bytes | 75.80 MB | 75.80 MB |

The memory reduction concerns **peak construction working set**, not a reduction
in retained index size. All eight full ordered comparisons matched the linear
reference. See [Benchmarks](docs/BENCHMARKS.md) for every query, hardware provenance,
sample counts, candidate limits, raw results and limitations.

## Installation

Download the Windows x64 portable archive or desktop installer from
[v0.2.0](https://github.com/styayur/local-context-engine/releases/tag/v0.2.0).
Verify the download against `SHA256SUMS.txt`. Extract the portable archive and run
`localsearch.exe`; add its directory to `PATH` if you want the commands below.
The desktop requires WebView2. A successful build is not a substitute for a
working desktop smoke test on your Windows installation.

To build the CLI and MCP server from source:

```powershell
git clone https://github.com/styayur/local-context-engine.git
cd local-context-engine
git checkout v0.2.0
cargo build --release --locked
.\target\release\localsearch.exe --help
```

Use a current stable Rust MSVC toolchain and Visual Studio C++ Build Tools on
Windows. [Development](docs/DEVELOPMENT.md) covers desktop prerequisites and CI.
An unprivileged directory scan is available without installing the optional
index service. Read the [MFT visibility boundary](docs/SECURITY.md#mft-and-permissions)
before choosing that service on a shared machine.

## Usage

```powershell
localsearch --providers
localsearch --backend scan "type:file ext:pdf"
localsearch --explain "ext:pdf report"
localsearch "prefix:read"
localsearch "type:process python"
localsearch "type:service state:running"
localsearch --index-status
```

Search commands return `0` for results, `1` for a valid query with no matches,
and `2` for an operation that could not complete. Relative time filters and live
providers naturally change with the clock and system state. Use the
[Query Engine](docs/QUERY_ENGINE.md), [DSL reference](docs/DSL.md) and
[MCP guide](docs/MCP.md) for filtering, JSON output and local agent integration.

## Security and privacy

Search does not upload data or index file contents. Cached paths, metadata,
settings and optional usage history remain sensitive local data. Activating a
result opens it through Windows; process termination requires explicit
confirmation. The optional elevated service can reveal filenames that a normal
directory walk cannot list. It does not grant permission to read their contents.
See [Security](docs/SECURITY.md) and [vulnerability reporting](SECURITY.md).

## Development

Read [Development](docs/DEVELOPMENT.md) and [Contributing](CONTRIBUTING.md) for
checks and extension points. [Roadmap](docs/ROADMAP.md) separates release gates
from future work; [Changelog](CHANGELOG.md) describes user-visible changes.

Repository license: [MPL-2.0](LICENSE). See [license notes](THIRD_PARTY_LICENSES.md)
for the remaining desktop package metadata inconsistency before v1.0.0.
LCE runs independently and has no runtime or
Rust crate dependency on CodeForge Engine.
