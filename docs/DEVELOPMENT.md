# Development

## Environment

Use Windows x64, current stable Rust MSVC, Visual Studio C++ Build Tools, Node 22
(the CI version) and npm. Desktop requires WebView2. Cargo declares Rust 1.82,
but current CI tests stable, not that minimum; minimum-toolchain compatibility
must be checked before a v1.0 promise. Linux is not a supported runtime backend.

## Build and existing gates

```powershell
cargo build --release --locked
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cd apps/desktop
npm ci
npm run lint
npm run typecheck
npm run build
npm run tauri:build
```

Workspace output is under `target/release/`, including CLI, MCP and optional
index-service binaries. Installer output is under `target/release/bundle/`.
Default Cargo members keep the desktop dependency stack out of ordinary tests;
the existing desktop CI checks its crate separately. CI also runs CodeForge
Regression Gate. That external **development check** is not a runtime/crate
dependency between the projects and does not relax search tests.

## Documentation and contribution

Keep English and Chinese README capability/version statements aligned. Run the
[documentation checks](DOCUMENTATION_CHECKS.md) and verify CLI examples against
`--help`. Release descriptions must distinguish Stable, Experimental and Future.
Follow [Contributing](../CONTRIBUTING.md) for provider and translation changes.
Historical architecture, delivery notes and raw benchmark captures are retained
in the [archive](archive/README.md); they are not current installation guidance.

## Release readiness

[Release preparation](RELEASE_PREPARATION.md) lists remaining gates. The draft
[release notes](releases/v1.0.0.md) do not change Cargo/package versions or create
a tag. Desktop build success alone does not establish successful rendering on a
real Windows machine.
