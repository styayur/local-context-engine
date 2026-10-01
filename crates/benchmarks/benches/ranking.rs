//! Ranking and query compilation benchmarks.
//!
//! Ranking runs over the candidate set the file index produced, which is capped
//! at `CANDIDATE_CAP`, so these numbers describe a bounded amount of work no
//! matter how large the index is.

use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use lce_benchmarks::synthetic_store;
use query_dsl::QueryCompiler as _;
use query_dsl::RuleBasedCompiler;
use ranking::HeuristicRanker;
use search_core::{LocalEntity, Ranker, SearchQuery};
use windows_files::{collect_candidates, CANDIDATE_CAP};

const SIZES: [usize; 3] = [10_000, 100_000, 1_000_000];

fn candidates(size: usize, text: &str) -> Vec<LocalEntity> {
    let store = synthetic_store(size, 0x0BAD_C0DE_1234_5678);
    let query = SearchQuery::plain(text);
    collect_candidates(&store, &query, CANDIDATE_CAP)
}

fn bench_ranking(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("ranking/heuristic");
    for size in SIZES {
        let entities = candidates(size, "report");
        let query = SearchQuery::plain("report");
        let ranker = HeuristicRanker::default();
        group.throughput(Throughput::Elements(entities.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &entities,
            |bencher, entities| {
                bencher.iter(|| {
                    let mut scored = 0usize;
                    for entity in entities {
                        if ranker.score(black_box(&query), entity).is_some() {
                            scored += 1;
                        }
                    }
                    black_box(scored)
                });
            },
        );
    }
    group.finish();
}

fn bench_ranking_with_usage(criterion: &mut Criterion) {
    use std::sync::{Arc, RwLock};

    let mut usage = ranking::UsageIndex::default();
    let mut group = criterion.benchmark_group("ranking/with_usage");
    for size in [10_000usize, 100_000] {
        let entities = candidates(size, "report");
        for (index, entity) in entities.iter().take(64).enumerate() {
            for _ in 0..3 {
                usage.record("report", &format!("{}-{index}", entity.id()));
            }
        }
        let ranker = HeuristicRanker::default().with_usage(Arc::new(RwLock::new(usage.clone())));
        let query = SearchQuery::plain("report");
        group.throughput(Throughput::Elements(entities.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &entities,
            |bencher, entities| {
                bencher.iter(|| {
                    let mut scored = 0usize;
                    for entity in entities {
                        if ranker.score(black_box(&query), entity).is_some() {
                            scored += 1;
                        }
                    }
                    black_box(scored)
                });
            },
        );
    }
    group.finish();
}

fn bench_compile(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("query_compiler");
    let compiler = RuleBasedCompiler::new();
    let cases = [
        ("plain", "vscode"),
        ("dsl", "type:file ext:rs modified:<24h sort:size-desc"),
        ("nl-zh", "最近的 pdf"),
        ("nl-en", "recent PDFs"),
        ("nl-process", "正在运行的 python"),
    ];
    for (name, input) in cases {
        group.bench_with_input(
            BenchmarkId::from_parameter(name),
            input,
            |bencher, input| {
                bencher.iter(|| black_box(compiler.compile(black_box(input))));
            },
        );
    }
    group.finish();
}

fn bench_full_provider_path(criterion: &mut Criterion) {
    // The one benchmark that measures what a keystroke actually costs: walk the
    // index, collect candidates, rank them.
    let mut group = criterion.benchmark_group("end_to_end/query");
    for size in SIZES {
        let store = synthetic_store(size, 0xFEED_FACE_CAFE_BEEF);
        let ranker = HeuristicRanker::default();
        let query = SearchQuery::plain("report");
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &store,
            |bencher, store| {
                bencher.iter(|| {
                    let found = collect_candidates(store, black_box(&query), CANDIDATE_CAP);
                    let mut total = 0.0f32;
                    for entity in &found {
                        if let Some(ranked) = ranker.score(&query, entity) {
                            total += ranked.score;
                        }
                    }
                    black_box((found.len(), total))
                });
            },
        );
    }
    group.finish();
}

fn bench_provider_stats(criterion: &mut Criterion) {
    let store = synthetic_store(100_000, 5);
    let mut group = criterion.benchmark_group("file_store/stats");
    group.bench_function("100000", |bencher| {
        bencher.iter(|| {
            black_box((
                store.file_count(),
                store.directory_count(),
                store.memory_bytes(),
            ))
        });
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(20)
        .measurement_time(Duration::from_secs(6))
        .warm_up_time(Duration::from_secs(1));
    targets =
        bench_ranking,
        bench_ranking_with_usage,
        bench_compile,
        bench_full_provider_path,
        bench_provider_stats
}
criterion_main!(benches);
