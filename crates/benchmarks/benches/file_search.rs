//! File index search benchmarks.
//!
//! These exercise `windows_files::collect_candidates` — the exact function the
//! provider calls — so the numbers describe the shipped code path rather than a
//! simplified stand-in.
//!
//! ```bash
//! cargo bench -p lce-benchmarks --bench file_search
//! # a single corpus size:
//! cargo bench -p lce-benchmarks --bench file_search -- 100000
//! ```

use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use lce_benchmarks::synthetic_store;
use search_core::{Filter, SearchQuery, SizeOp, Sort, SortKey};
use windows_files::{collect_candidates, FileStore, CANDIDATE_CAP};

/// Corpus sizes from the project's benchmark contract.
const SIZES: [usize; 3] = [10_000, 100_000, 1_000_000];

fn store_for(size: usize) -> FileStore {
    synthetic_store(size, 0x1234_5678_9ABC_DEF0)
}

fn bench_exact(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("file_search/exact");
    for size in SIZES {
        let store = store_for(size);
        let query = SearchQuery::plain("readme");
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &store,
            |bencher, store| {
                bencher.iter(|| {
                    black_box(collect_candidates(store, black_box(&query), CANDIDATE_CAP))
                });
            },
        );
    }
    group.finish();
}

fn bench_prefix(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("file_search/prefix");
    for size in SIZES {
        let store = store_for(size);
        let query = SearchQuery::plain("re");
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &store,
            |bencher, store| {
                bencher.iter(|| {
                    black_box(collect_candidates(store, black_box(&query), CANDIDATE_CAP))
                });
            },
        );
    }
    group.finish();
}

fn bench_substring(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("file_search/substring");
    for size in SIZES {
        let store = store_for(size);
        let query = SearchQuery::plain("port");
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &store,
            |bencher, store| {
                bencher.iter(|| {
                    black_box(collect_candidates(store, black_box(&query), CANDIDATE_CAP))
                });
            },
        );
    }
    group.finish();
}

fn bench_rare_substring(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("file_search/rare_substring");
    for size in SIZES {
        let store = store_for(size);
        // Matches almost nothing, which is the realistic typing case: the scan
        // still has to visit every record, but materialises almost none.
        let query = SearchQuery::plain("zzqxjw");
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &store,
            |bencher, store| {
                bencher.iter(|| {
                    black_box(collect_candidates(store, black_box(&query), CANDIDATE_CAP))
                });
            },
        );
    }
    group.finish();
}

fn bench_fuzzy(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("file_search/fuzzy");
    for size in SIZES {
        let store = store_for(size);
        // A subsequence query: no contiguous occurrence exists, so the fuzzy
        // matcher runs on every candidate.
        let query = SearchQuery::plain("rpt");
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &store,
            |bencher, store| {
                bencher.iter(|| {
                    black_box(collect_candidates(store, black_box(&query), CANDIDATE_CAP))
                });
            },
        );
    }
    group.finish();
}

fn bench_filters(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("file_search/filters");
    for size in SIZES {
        let store = store_for(size);
        let query = SearchQuery::plain("report")
            .with_filter(Filter::Extension("pdf".into()))
            .with_filter(Filter::Drive('C'))
            .with_filter(Filter::Size(search_core::SizeFilter {
                op: SizeOp::GreaterThan,
                bytes: 1_048_576,
            }));
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &store,
            |bencher, store| {
                bencher.iter(|| {
                    black_box(collect_candidates(store, black_box(&query), CANDIDATE_CAP))
                });
            },
        );
    }
    group.finish();
}

fn bench_path_filter(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("file_search/path_filter");
    for size in [10_000usize, 100_000] {
        let store = store_for(size);
        let query = SearchQuery::plain("report").with_filter(Filter::Path("docs".into()));
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &store,
            |bencher, store| {
                bencher.iter(|| {
                    black_box(collect_candidates(store, black_box(&query), CANDIDATE_CAP))
                });
            },
        );
    }
    group.finish();
}

fn bench_sort(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("file_search/sort");
    for size in SIZES {
        let store = store_for(size);
        let query = SearchQuery::plain("report")
            .with_sort(Sort::new(SortKey::Size, search_core::SortDirection::Desc));
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &store,
            |bencher, store| {
                bencher.iter(|| {
                    black_box(collect_candidates(store, black_box(&query), CANDIDATE_CAP))
                });
            },
        );
    }
    group.finish();
}

fn bench_store_load(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("file_store/load");
    for size in SIZES {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |bencher, size| {
            bencher.iter(|| black_box(synthetic_store(*size, 0x1234_5678_9ABC_DEF0)));
        });
    }
    group.finish();
}

fn bench_extension_index(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("file_store/extension_lookup");
    for size in SIZES {
        let store = store_for(size);
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &store,
            |bencher, store| {
                bencher.iter(|| {
                    let mut hits = 0usize;
                    for index in 0..u32::try_from(store.len()).unwrap_or(0) {
                        if store.extension(index) == Some("pdf") {
                            hits += 1;
                        }
                    }
                    black_box(hits)
                });
            },
        );
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(20)
        .measurement_time(Duration::from_secs(6))
        .warm_up_time(Duration::from_secs(1));
    targets =
        bench_exact,
        bench_prefix,
        bench_substring,
        bench_rare_substring,
        bench_fuzzy,
        bench_filters,
        bench_path_filter,
        bench_sort,
        bench_store_load,
        bench_extension_index
}
criterion_main!(benches);
