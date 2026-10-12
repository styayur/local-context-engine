# Lightweight upgrade audit and plan

Baseline: origin/main, Rust 1.98.1, Windows, 2026-10-12. Existing modules already
provide extension/prefix/path-trigram accelerators, linear differential tests,
MFT/USN mutation handling, versioned MessagePack persistence and Criterion suites.
No additional crate or dependency is needed.

Ordered implementation (benefit / complexity / coupling):

1. Fix long-prefix false negatives and retain usable indexed predicates when
   short/common terms or path filters occur (high / low / local planner).
2. Order candidate sources by cardinality; distinguish absent from deliberately
   dropped trigrams; prune common postings during construction and release excess
   prefix capacity (high / medium / accelerator only).
3. Run the same million-record probe before/after, plus mutation, persistence,
   Unicode, long-prefix and capped-order differential tests. Keep the DSL,
   authoritative verifier, record ordering, permissions and journal boundaries.

Posting compression decision: delta/varint encoding can reduce retained u32
postings, but adds decode work, mutation complexity and a cache format migration.
First remove transient common postings; defer compressed storage until retained
postings dominate measured RSS. Prefix keys keep their persisted 24-byte shape.

Optional full-text/semantic search: use a separately enabled existing Provider
implementation with explicit capabilities and source locations. No registration,
model loading, file-content indexing, network access or process startup in the
default filename search path. A content score must never alter filename DSL
matching or permission filtering.

References are design inputs only; no upstream code is copied:
- [zvec-grep](https://github.com/zvec-ai/zvec-grep), Apache-2.0: separate search
  modes behind interfaces; exact verification and source attribution.
- [Renovate](https://github.com/renovatebot/renovate), AGPL-3.0: bounded,
  deterministic work. No framework dependency or redistributed upstream code.

Measurement command: `cargo run --release --locked -p lce-benchmarks --example
planner_probe -- 1000000`. Fixed seed, three warmups, forty samples, nearest-rank
P95; full ordered result equality is checked separately from capped timing.
Estimated index bytes are not RSS. Record process peak working set separately.

## Measured results (paired local run)

Windows release build; synthetic one-million-record fixture. Timing uses a cap
of 2,000; equality compares every result in record order. Forty samples after
three warmups per query. Other local work can affect timing; these are local
observations, not CI thresholds.

| Query | Before mean / P95 ms | After mean / P95 ms |
|---|---:|---:|
| `zzqxjw-nothing-matches` | 106.4053 / 114.7028 | 0.0022 / 0.0025 |
| `ext:pdf re` | 30.0422 / 34.3426 | 7.4280 / 9.3880 |
| `ext:pdf report` | 89.3596 / 99.7413 | 20.9528 / 23.1421 |
| `ext:pdf path:projects` | 267.6831 / 286.4835 | 0.0019 / 0.0020 |
| `prefix:design report` | 87.8492 / 93.3898 | 19.8860 / 21.8997 |
| `design-99999` | 2.7867 / 3.6018 | 2.0751 / 2.2992 |
| `report` | 8.3357 / 9.6114 | 8.4723 / 9.8673 |
| `prefix:read` | 4.0103 / 4.8976 | 3.3955 / 3.7737 |

before: build_ms=11934.3107,estimated_index_bytes=75796186,records=1000000; peak working set 1,005,248,512 bytes; standalone probe binary 289,792 bytes.

after: build_ms=8908.7187,estimated_index_bytes=75796186,records=1000000; peak working set 258,981,888 bytes; standalone probe binary 292,352 bytes.

Retained estimated index bytes are unchanged (75,796,186); the memory benefit
is peak construction memory from early posting pruning and retaining directory
paths only. The probe binary grows by 2,560 bytes. No product CLI binary size
claim is made from that probe. The common `report` fallback has a small timing
variation, so no speedup is claimed for that shape. All eight full-result
comparisons pass. Cargo.toml and Cargo.lock are unchanged: zero new dependencies.

Validation: original cargo test baseline passed; updated cargo test and
cargo clippy --all-targets --all-features -- -D warnings passed. The existing
CodeForge Regression Gate and Windows/desktop CI also run on the PR.
