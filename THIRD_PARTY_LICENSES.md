# Third-party licenses

Local Context Engine is [MPL-2.0](LICENSE) licensed, matching the Rust workspace
metadata. The [license text](https://www.mozilla.org/en-US/MPL/2.0/) governs the
project; an earlier README incorrectly identified it as MIT.

The desktop package and its root lockfile entry now declare MPL-2.0, matching
the existing repository LICENSE, Rust workspace and desktop Rust crate. This
corrects stale metadata after the earlier MPL adoption; no separate desktop
license or external non-bot contributor was found in the review. Earlier
versions retain any permissions already granted under their own terms. This
alignment does not relicense dependencies or retroactively change releases.

Cargo.lock and the desktop package lockfile record the dependency inventory.
Dependencies retain their own licenses. This document is not a complete
installer notice bundle: review the exact linked/shipped components, upstream
notices and source obligations before distributing a release artifact.

Renovate and zvec-grep were design references, not framework dependencies. No
upstream implementation was copied or bundled in the planner upgrade. There
is no runtime or Rust crate dependency on CodeForge Engine; its existing CI
Regression Gate is an external development check.
