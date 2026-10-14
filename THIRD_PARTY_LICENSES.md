# Third-party licenses

Local Context Engine is [MPL-2.0](LICENSE) licensed, matching the Rust workspace
metadata. The [license text](https://www.mozilla.org/en-US/MPL/2.0/) governs the
project; an earlier README incorrectly identified it as MIT.

An unresolved packaging inconsistency remains: the desktop package manifest and
its root lockfile entry still say MIT. No separate desktop LICENSE was found.
Resolve and review that metadata against the intended license before v1.0.0;
this documentation change does not silently relicense files or alter manifests.

Cargo.lock and the desktop package lockfile record the dependency inventory.
Dependencies retain their own licenses. This document is not a complete
installer notice bundle: review the exact linked/shipped components, upstream
notices and source obligations before distributing a release artifact.

Renovate and zvec-grep were design references, not framework dependencies. No
upstream implementation was copied or bundled in the planner upgrade. There
is no runtime or Rust crate dependency on CodeForge Engine; its existing CI
Regression Gate is an external development check.
