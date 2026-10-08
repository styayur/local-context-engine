# README evidence capture — 2026-10-08

The README application icon is identity material, not a software screenshot.
Its original packaged files remain unchanged. Existing benchmark documentation
is retained; the headline no longer makes an unconditional millisecond claim.
The Chinese license badge is aligned with the actual MPL-2.0 LICENSE.

A real Windows screenshot was attempted from the existing release executable and
from the current main source rebuilt with `npm run tauri:build -- --no-bundle`.
Both native windows rendered a blank WebView in this environment. No simulated
UI, composite or AI-generated screenshot was substituted. A functional desktop
capture remains pending; this is an environment observation, not proof that every
released installation fails.

Repeat with isolated LOCALAPPDATA and a fixture-only configuration; disable private
drive indexing, use a disposable fixture drive/directory and inspect every visible
path before capturing the native window. Record source SHA, platform and data
fixture. Publish only a working interface demonstrating current behavior.

The real CLI `target/release/localsearch.exe --help` ran successfully after the
source build. Sanitized excerpt (not a search over private files):

```text
Usage: localsearch.exe [OPTIONS] [QUERY]...

Arguments:
  [QUERY]...  Search terms: plain text, natural language, or Local Search DSL
```

The README already contains shortest CLI/DSL examples and an architecture diagram;
they remain the available engineering evidence while the GUI capture is pending.
