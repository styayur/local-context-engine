# Roadmap

## Stable

CLI and local MCP discovery, explicit DSL, rule-based query compilation,
explainable ranking, indexed file candidate planning, unprivileged scan and live
Windows providers have implementations and regression coverage. Stability here
is scoped to those workflows and supported environments, not a v1 API promise.

## Experimental

The optional privileged MFT/USN service and Desktop need deployment acceptance
across the intended Windows/filesystem/WebView2 matrix. MFT filename exposure,
missing file-size metadata, per-volume journal health and shared-user access
must remain documented. The changes feed is volume-level, not per-file.

## v1.0 release gates

- Align Cargo, desktop package and installer versions in a separate release change.
- Validate the declared Rust minimum or document a newly tested minimum.
- Run existing CI, Regression Gate, package checksums and fresh-install smoke tests.
- Capture real desktop search behavior: an earlier environment showed a blank
  WebView; a successful installer build does not close that observation.
- Validate service installation, ACLs, fallback, journal rollover and uninstall
  on representative machines before describing the service as stable.

## Future

Evaluate compressed postings only when retained memory justifies decoding,
mutation and cache-format costs. Improve metadata completeness and application
package discovery. Any full-text, semantic or model-based compiler extension
must be explicit opt-in and preserve the filename DSL, permission checks and
default offline path. These are directions, not scheduled capabilities.
