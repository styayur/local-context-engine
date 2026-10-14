# Benchmarks

This report uses the existing paired million-record planner probe. It measures
in-memory file candidate selection and accelerator construction, not end-to-end
desktop responsiveness, filesystem enumeration or live provider latency.

## Method and environment

| Item | Recorded value |
|---|---|
| Platform/toolchain | Windows, Rust 1.98.1, release build |
| Dataset | 1,000,000 synthetic records; seed `0x1234_5678_9ABC_DEF0` |
| Timing | 3 warmups and 40 samples per query; arithmetic mean and nearest-rank P95 |
| Timed result cap | 2,000 matches; not an exhaustive full-result latency measurement |
| Correctness | Separate uncapped comparison of every result ID in record order with the linear reference |
| Memory | Windows process peak working set; retained accelerator bytes are a separate internal estimate |
| Provenance | October 2026 paired local captures linked below |

The paired capture does **not** record the CPU, RAM, disk or full target triple.
Those missing fields limit reproducibility and must be recorded in a new
release-quality run. The older Criterion report records a Ryzen 7 255 H,
22.8 GB RAM, KY3000-512GB NVMe, Windows 11 build 26200 and GNU toolchain, but it
is a different harness/workload and must not supply assumed hardware metadata
for this pair. Its original report remains in the archive.

## All query results

Times are milliseconds; zero-result rows are explicit so a fast negative lookup
is not mistaken for materializing a large result set.

| Query | Results | Before mean / P95 | After mean / P95 |
|---|---:|---:|---:|
| `zzqxjw-nothing-matches` | 0 | 106.4053 / 114.7028 | 0.0022 / 0.0025 |
| `ext:pdf re` | 10,445 | 30.0422 / 34.3426 | 7.4280 / 9.3880 |
| `ext:pdf report` | 3,441 | 89.3596 / 99.7413 | 20.9528 / 23.1421 |
| `ext:pdf path:projects` | 0 | 267.6831 / 286.4835 | 0.0019 / 0.0020 |
| `prefix:design report` | 0 | 87.8492 / 93.3898 | 19.8860 / 21.8997 |
| `design-99999` | 0 | 2.7867 / 3.6018 | 2.0751 / 2.2992 |
| `report` | 50,076 | 8.3357 / 9.6114 | 8.4723 / 9.8673 |
| `prefix:read` | 49,998 | 4.0103 / 4.8976 | 3.3955 / 3.7737 |

All eight uncapped ordered comparisons passed. Common `report` slightly worsened
in this run; no speedup is claimed for that query. These are local observations,
not confidence intervals or portable latency guarantees.

## Construction and resources

| Measurement | Before | After |
|---|---:|---:|
| Accelerator construction, ms | 11,934.3107 | 8,908.7187 |
| Construction process peak working set, bytes | 1,005,248,512 | 258,981,888 |
| Estimated retained accelerator bytes | 75,796,186 | 75,796,186 |
| Standalone probe binary, bytes | 289,792 | 292,352 |
| Separately measured CLI binary, bytes | 2,346,496 | 2,349,568 |

Early pruning reduces transient construction memory; retained accelerator size
did not decrease. Working set is the Windows resident-memory analogue, not a
portable RSS/heap measurement. The retained estimate excludes the FileStore and
other process memory. The old approximately 69 MB FileStore figure was an
internal accounting estimate, not measured process RSS. Cargo lockfile inventory
remains 492 packages (475 external); the optimization added no dependencies.

## Reproduce and verify

```powershell
cargo run --release --locked -p lce-benchmarks --example planner_probe -- 1000000
cargo bench -p lce-benchmarks --bench accelerators
$env:LCE_BENCH_1M = "1"
cargo bench -p lce-benchmarks --bench accelerators
Remove-Item Env:LCE_BENCH_1M
cargo test -p windows-files
```

The [probe source](../crates/benchmarks/examples/planner_probe.rs) specifies the
queries, cap and equality oracle. Record commit, target, toolchain, hardware,
OS, background load and process-memory sampling method for new measurements.
Criterion results use a different statistical method and must be reported
separately. Differential/index tests also cover mutation, persistence, long
prefixes, Unicode and capped order; a synthetic probe alone is not coverage of
all real-world names, permissions or journal failures.

## Raw evidence and limitations

- [Before CSV](archive/benchmark-history/planner-20261012/planner-before.csv) and [after CSV](archive/benchmark-history/planner-20261012/planner-after.csv).
- [Before process capture](archive/benchmark-history/planner-20261012/planner-before-process.json) and [after process capture](archive/benchmark-history/planner-20261012/planner-after-process.json).
- [Before build output](archive/benchmark-history/planner-20261012/planner-before-build.txt) and [after build output](archive/benchmark-history/planner-20261012/planner-after-build.txt).
- [Original Criterion history](archive/benchmark-history/benchmarks-before-v1.md), retained as historical evidence, not a current comparison.

The historical prefix/extension comparison reused a different rare-substring
baseline; it cannot support a like-for-like speedup claim. Historical memory
extrapolations are not measured million-file RSS. Real disk scans, ACL behavior,
MFT startup, USN updates and desktop rendering need separate deployment tests.
No new benchmark was run for this documentation-only revision.
