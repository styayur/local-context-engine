# Contributing

## Ground rules

1. **The search core stays deterministic.** No model, no embedding and no
   network call may appear on the search path. An AI may sit in front of the
   query compiler; it may never sit inside retrieval.
2. **No fake UI.** A button either does the thing or is not there. There are no
   placeholder features and no mock results.
3. **No `unwrap()`/`expect()`/`todo!()` in production code.** Tests may use
   them; `#[cfg(test)]` modules already allow the relevant lints.
4. **Errors are structured.** Add a variant with a stable code and a user-facing
   hint; never let a raw OS error reach the interface.

## Getting set up

```powershell
git clone https://github.com/styayur/local-context-engine
cd local-context-engine

# Rust workspace
cargo test

# Desktop shell
cd apps/desktop
npm install
npm run tauri:dev
```

You need the MSVC toolchain (Visual Studio Build Tools with "Desktop development
with C++") plus Node 20 or newer. The desktop shell also needs WebView2, which
ships with Windows 11 and is installed by the NSIS bundle otherwise.

## Before you open a pull request

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test

cd apps/desktop
npm run lint
npm run typecheck
npm run build
```

Everything above runs in CI on `windows-latest`; a red CI run is the fastest way
to get a review comment, so please run it locally first.

## Where things live

| path                          | what it owns                                  |
|-------------------------------|-----------------------------------------------|
| `crates/search-core`          | entity/query/result model, engine, fuzzy match |
| `crates/query-dsl`            | the DSL parser and the NL compiler            |
| `crates/ranking`              | heuristic scoring and usage memory            |
| `crates/windows-*`            | one OS domain each                            |
| `crates/search-daemon`        | the composition root every front end uses     |
| `crates/search-cli`           | `localsearch.exe`                             |
| `crates/search-mcp`           | the MCP server                                |
| `apps/desktop`                | Tauri shell + React UI                        |

If you are adding a new searchable domain, add a provider crate that implements
`EntityProvider`. Do not add search logic to the UI, the CLI or the MCP server:
they all call `SearchService`.

## Adding a translation

Copy `apps/desktop/src/i18n/en-US.json`, translate the values, and add the locale
to `apps/desktop/src/i18n/index.ts`. `missingKeys()` tells you what is left.
Every string — including errors, empty states, tooltips and context menus —
belongs in the bundle; nothing is hard-coded in a component.

## Commit messages

Conventional commits, please: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`,
`chore:`. The release workflow generates its notes from them.