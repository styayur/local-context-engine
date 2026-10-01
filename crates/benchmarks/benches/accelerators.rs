//! Accelerator benchmarks: the "after" side of the v0.2 comparison.
//!
//! `file_search.rs` measures the pre-v0.2 full scan. This suite measures the
//! same queries through the planner and the accelerators, plus what the
//! accelerators cost to build and to hold in memory.
//!
//! ```bash
//! cargo bench -p lce-benchmarks --bench accelerators
//! ```

use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use lce_benchmarks::synthetic_store;
use search_core::{Filter, SearchQuery};
use windows_files::{
    collect_candidates_with_plan, explain_plan, SearchAccelerators, CANDIDATE_CAP,
};

/// Corpus sizes measured by default.
///
/// A one million entry run is available with `LCE_BENCH_1M=1`, but it is not
/// the default: building the accelerators for a million *paths* is a real cost
/// (see `docs/BENCHMARKS.md`), and a suite that takes an hour is a suite nobody
/// runs.
fn sizes() -> Vec<usize> {
    let mut sizes = vec![10_000, 100_000];
    if std::env::var_os("LCE_BENCH_1M").is_some() {
        sizes.push(1_000_000);
    }
    sizes
}

struct Fixture {
    store: windows_files::FileStore,
    accelerators: SearchAccelerators,
}

fn fixture(size: usize) -> Fixture {
    let store = synthetic_store(size, 0x1234_5678_9ABC_DEF0);
    let accelerators = SearchAccelerators::build(&store);
    Fixture {
        store,
        accelerators,
    }
}

/// Build every fixture once and reuse it across query shapes.
///
/// Rebuilding a million entry corpus inside each benchmark would dominate the
/// measurement and make the suite unusable.
fn fixtures() -> Vec<(usize, Fixture)> {
    sizes()
        .into_iter()
        .map(|size| (size, fixture(size)))
        .collect()
}

fn run(fixture: &Fixture, query: &SearchQuery) -> usize {
    let (entities, _info) =
        collect_candidates_with_plan(&fixture.store, &fixture.accelerators, query, CANDIDATE_CAP);
    entities.len()
}

/// Every query shape the project promises to accelerate.
fn queries() -> Vec<(&'static str, SearchQuery)> {
    vec![
        ("exact", SearchQuery::plain("readme")),
        ("prefix_filter", {
            // `prefix:` is the predicate the prefix table answers exactly.
            let mut query = SearchQuery::plain("");
            query.filters.push(Filter::NamePrefix("read".into()));
            query
        }),
        ("extension", {
            let mut query = SearchQuery::plain("");
            query.filters.push(Filter::Extension("pdf".into()));
            query
        }),
        ("common_substring", SearchQuery::plain("report")),
        ("rare_substring", SearchQuery::plain("design-99999")),
        (
            "negative_substring",
            SearchQuery::plain("zzqxjw-nothing-matches"),
        ),
        ("fuzzy", SearchQuery::plain("rpt")),
        ("combined", {
            let mut query = SearchQuery::plain("report");
            query.filters.push(Filter::Extension("pdf".into()));
            query
        }),
    ]
}

fn bench_planner(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("accelerated/query");
    for (size, fixture) in fixtures() {
        for (name, query) in queries() {
            let plan = explain_plan(&fixture.store, &fixture.accelerators, &query).plan;
            // A query the planner cannot accelerate is still measured, so the
            // table shows the fallback rather than hiding it.
            let label = format!("{size}/{name}/{plan}");
            group.throughput(Throughput::Elements(size as u64));
            group.bench_with_input(
                BenchmarkId::new("query", label),
                &fixture,
                |bencher, fixture| {
                    bencher.iter(|| black_box(run(fixture, black_box(&query))));
                },
            );
        }
    }
    group.finish();
}

fn bench_build(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("accelerated/build");
    group.sample_size(10);
    for size in sizes() {
        let store = synthetic_store(size, 0x1234_5678_9ABC_DEF0);
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &store,
            |bencher, store| {
                bencher.iter(|| black_box(SearchAccelerators::build(store)));
            },
        );
    }
    group.finish();
}

fn bench_planning(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("accelerated/planning");
    for (size, fixture) in fixtures() {
        let query = SearchQuery::plain("design-99999");
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &fixture,
            |bencher, fixture| {
                bencher.iter(|| {
                    black_box(explain_plan(
                        &fixture.store,
                        &fixture.accelerators,
                        black_box(&query),
                    ))
                });
            },
        );
    }
    group.finish();
}

/// Report the plan and the memory cost of every corpus size once, so the
/// numbers in the docs can be checked against the run.
fn report_shape(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("accelerated/shape");
    group.sample_size(10);
    for size in sizes() {
        let fixture = fixture(size);
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &fixture,
            |bencher, fixture| {
                bencher.iter(|| {
                    let stats = fixture.accelerators.stats();
                    black_box((
                        stats.trigrams,
                        stats.trigrams_dropped,
                        stats.trigram_postings,
                        stats.memory_bytes,
                    ))
                });
            },
        );
    }
    group.finish();
}

/// A guard so a plan regression shows up as a failure rather than as a slower
/// number nobody reads: the rare query must plan through the trigram index.
#[test]
fn rare_queries_plan_through_the_trigram_index() {
    let fixture = fixture(100_000);
    let plan = explain_plan(
        &fixture.store,
        &fixture.accelerators,
        &SearchQuery::plain("design-99999"),
    );
    assert_eq!(
        plan.plan,
        windows_files::SearchPlan::TrigramLookup,
        "{plan:?}"
    );
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .measurement_time(Duration::from_secs(3))
        .warm_up_time(Duration::from_millis(500));
    targets = bench_planner, bench_build, bench_planning, report_shape
}
criterion_main!(benches);
