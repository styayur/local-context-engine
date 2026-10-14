# v1.0.0 release preparation

**Status: draft, not ready to tag solely on documentation approval.** Runtime,
frontend and desktop package versions remain 0.2.0. This documentation change
does not create a compatibility guarantee, tag or release binary.

## Prepared in this documentation stage

- User-facing README, canonical guides and Stable/Experimental/Future labels.
- Evidence-preserving history archive and honest performance methodology.
- [Changelog](../CHANGELOG.md) and [user-facing release draft](releases/v1.0.0.md).
- [Documentation verification procedure](DOCUMENTATION_CHECKS.md).

## Required before publication

- Merge the reviewed documentation PR only after its original CI and Regression Gate are green.
- Align Cargo/workspace, frontend and Tauri package metadata to 1.0.0 in a separately reviewed release change, including generated lockfile version entries as needed.
- Validate CLI/protocol/cache compatibility and state the supported platform/toolchain policy.
- Build final artifacts from the exact reviewed tag candidate; verify installation, uninstall, version output and published checksums.
- Review license notices against final dependency inventories and bundled artifacts; optional external tools are not licensed by this project.

- Validate Rust 1.82 or revise the declared minimum in a separate code/release change.
- Resolve desktop package/lockfile root license metadata (MIT) against repository LICENSE and Cargo metadata (MPL-2.0); no separate desktop LICENSE was found.
- Repeat the million-record probe with complete hardware/target metadata; report all queries and process memory separately from index estimates.
- Validate desktop rendering on a clean Windows/WebView2 installation; historical notes include a blank-window observation.
- Exercise optional service installation/uninstallation, named-pipe ACLs, multi-volume journal recovery and filename visibility under the intended user accounts.

## Release handling

The GitHub draft uses the proposed `v1.0.0` name but remains unpublished. Do not
push that tag or publish the draft until all release gates above have recorded
evidence. The draft contains no fabricated binary/validation claim. Passing a
documentation PR does not resolve packaging, MSRV or platform-acceptance work.
