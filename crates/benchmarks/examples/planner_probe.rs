//! Reproducible end-to-end planner probe; output is CSV, time is milliseconds.
use lce_benchmarks::synthetic_store;
use std::{hint::black_box, time::Instant};
use windows_files::{collect_candidates_linear, collect_candidates_with_plan, SearchAccelerators};

fn main() {
    let size = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000_000);
    let store = synthetic_store(size, 0x1234_5678_9ABC_DEF0);
    let start = Instant::now();
    let index = SearchAccelerators::build(&store);
    eprintln!(
        "build_ms={},estimated_index_bytes={},records={}",
        start.elapsed().as_secs_f64() * 1000.0,
        index.memory_bytes(),
        store.len()
    );
    println!("query,mean_ms,p95_ms,candidates,results,correct");
    for dsl in [
        "zzqxjw-nothing-matches",
        "ext:pdf re",
        "ext:pdf report",
        "ext:pdf path:projects",
        "prefix:design report",
        "design-99999",
        "report",
        "prefix:read",
    ] {
        let query = query_dsl::parse(dsl).query;
        let reference = collect_candidates_linear(&store, &query, usize::MAX);
        let (actual, info) = collect_candidates_with_plan(&store, &index, &query, usize::MAX);
        let correct = actual
            .iter()
            .map(|e| e.id())
            .eq(reference.iter().map(|e| e.id()));
        assert!(correct, "differential mismatch: {dsl}");
        let mut samples = Vec::new();
        for i in 0..43 {
            let start = Instant::now();
            black_box(collect_candidates_with_plan(
                &store,
                &index,
                black_box(&query),
                2000,
            ));
            if i >= 3 {
                samples.push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "{dsl},{:.6},{:.6},{},{},{}",
            samples.iter().sum::<f64>() / samples.len() as f64,
            samples[37],
            info.initial_candidates,
            actual.len(),
            correct
        );
    }
}
