//! End-to-end tests for the compile → search → rank pipeline, driven through
//! the same `SearchService` the CLI, the MCP server and the desktop UI use.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use search_core::{
    EntityProvider, EntityType, Filter, LocalEntity, RankedMatch, Ranker, SearchEngine,
    SearchQuery, SearchResult,
};
use search_daemon::{SearchOptions, SearchService, Settings, UsageIndex};

fn service() -> SearchService {
    SearchService::new(Settings::default(), UsageIndex::default())
}

#[test]
fn the_documented_mvp_queries_all_work() {
    let service = service();

    // 1. A plain word fans out to every provider and returns a response.
    let response = service.search("vscode", &SearchOptions::default());
    assert_eq!(response.query, "vscode");
    assert_eq!(response.compiled, "vscode");
    assert!(response.elapsed_ms >= 0.0);

    // 2. `type:process` narrows to processes.
    let response = service.search("type:process python", &SearchOptions::default());
    assert!(response
        .results
        .iter()
        .all(|result| result.entity_type == EntityType::Process));

    // 3. Natural language compiles before it searches.
    let outcome = service.search_detailed("最近的 pdf", &SearchOptions::default());
    assert_eq!(
        outcome.compiled.to_dsl(),
        "type:file ext:pdf sort:modified-desc"
    );
    assert_eq!(outcome.compiled.source.as_str(), "natural-language");

    // 4. An empty result set is not an error.
    let response = service.search("zzzqxjw-nothing-matches-this", &SearchOptions::default());
    assert!(response.results.is_empty());
}

#[test]
fn english_and_chinese_intents_produce_the_same_query() {
    let service = service();
    for (english, chinese) in [
        ("recent PDFs", "最近的 PDF"),
        ("running python processes", "正在运行的 python"),
        ("find VS Code", "找 VS Code"),
        ("yesterday modified rust files", "昨天修改的 rust 文件"),
    ] {
        let en = service.compile(english);
        let zh = service.compile(chinese);
        assert_eq!(en.to_dsl(), zh.to_dsl(), "`{english}` vs `{chinese}`");
    }
}

#[test]
fn a_type_filter_from_the_ui_reaches_the_engine() {
    let service = service();
    let response = service.search(
        "a",
        &SearchOptions {
            types: vec![EntityType::Service],
            limit: Some(5),
            explain: true,
        },
    );
    assert!(response
        .results
        .iter()
        .all(|result| result.entity_type == EntityType::Service));
}

#[test]
fn the_process_domain_is_searchable_end_to_end() {
    let service = service();
    let query = SearchQuery::plain("")
        .with_filter(Filter::Pid(std::process::id()))
        .with_types([EntityType::Process]);
    let response = service.search_query(&query);
    assert_eq!(response.total, 1);
    assert_eq!(response.results[0].entity_type, EntityType::Process);
    assert!(response.results[0].metadata.contains_key("memory_bytes"));
}

#[test]
fn results_are_ranked_best_first() {
    let service = service();
    let response = service.search(
        "node",
        &SearchOptions {
            limit: Some(20),
            ..Default::default()
        },
    );
    let scores: Vec<f32> = response.results.iter().map(|result| result.score).collect();
    assert!(
        scores.windows(2).all(|pair| pair[0] >= pair[1]),
        "scores must be non-increasing: {scores:?}"
    );
}

#[test]
fn explicit_sort_overrides_relevance() {
    let service = service();
    let outcome = service.search_detailed(
        "type:process sort:name-asc",
        &SearchOptions {
            limit: Some(10),
            ..Default::default()
        },
    );
    let names: Vec<String> = outcome
        .response
        .results
        .iter()
        .map(|result| result.display_name.to_lowercase())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

#[test]
fn index_status_is_always_answerable() {
    let service = service();
    let status = service.index_status();
    assert!(status.cache_path.ends_with(".lce-index"));
    assert_eq!(status.providers.len(), 5);
    for provider in &status.providers {
        assert!(!provider.name.is_empty());
        assert!(!provider.entity_types.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Engine-level tests with a synthetic provider, so fan-out and ranking can be
// asserted without depending on what happens to be running on this machine.
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct StaticProvider {
    name: &'static str,
    entities: Vec<LocalEntity>,
}

impl EntityProvider for StaticProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    fn scope(&self) -> search_core::SnapshotScope {
        search_core::SnapshotScope::Live
    }

    fn entity_types(&self) -> &'static [EntityType] {
        static TYPES: [EntityType; 1] = [EntityType::File];
        &TYPES
    }

    fn collect(&self, query: &SearchQuery) -> search_core::error::Result<Vec<LocalEntity>> {
        Ok(self
            .entities
            .iter()
            .filter(|entity| query.accepts_type(entity.entity_type()))
            .cloned()
            .collect())
    }
}

#[derive(Debug)]
struct NameRanker;

impl Ranker for NameRanker {
    fn score(&self, query: &SearchQuery, entity: &LocalEntity) -> Option<RankedMatch> {
        let text = query.text.as_deref()?.to_lowercase();
        if entity.name().to_lowercase().contains(&text) {
            Some(RankedMatch::bare(10.0))
        } else {
            None
        }
    }
}

fn file(name: &str) -> LocalEntity {
    LocalEntity::File(search_core::FileEntry {
        path: format!(r"C:\{name}"),
        name: name.to_string(),
        extension: search_core::extension_of(name).map(str::to_string),
        size: 1,
        modified: None,
        created: None,
        drive: Some('C'),
        file_id: None,
    })
}

#[test]
fn the_engine_fans_out_and_drops_non_matches() {
    let providers: Vec<std::sync::Arc<dyn EntityProvider>> =
        vec![std::sync::Arc::new(StaticProvider {
            name: "static",
            entities: vec![file("alpha.txt"), file("bravo.txt"), file("charlie.txt")],
        })];
    let engine = SearchEngine::new(providers, std::sync::Arc::new(NameRanker));
    let response = engine.search("alpha", &SearchQuery::plain("alpha"));
    assert_eq!(response.total, 1);
    assert_eq!(response.results[0].name, "alpha.txt");
    assert_eq!(response.timings.len(), 1);
    assert_eq!(response.timings[0].candidates, 3);
}

#[test]
fn the_engine_respects_the_limit_and_reports_truncation() {
    let providers: Vec<std::sync::Arc<dyn EntityProvider>> =
        vec![std::sync::Arc::new(StaticProvider {
            name: "static",
            entities: (0..10).map(|i| file(&format!("item-{i}.txt"))).collect(),
        })];
    let engine = SearchEngine::new(providers, std::sync::Arc::new(NameRanker));
    let mut query = SearchQuery::plain("item");
    query.limit = 4;
    let response = engine.search("item", &query);
    assert_eq!(response.results.len(), 4);
    assert!(response.truncated);
}

#[test]
fn an_entity_serialises_and_deserialises_losslessly() {
    let entity = file("report.pdf");
    let json = serde_json::to_string(&entity).expect("entities must serialise");
    let restored: LocalEntity = serde_json::from_str(&json).expect("entities must deserialise");
    assert_eq!(entity, restored);

    // The wire shape is part of the contract: the tag names the variant and the
    // payload is flattened into the same object.
    assert!(json.contains("\"kind\":\"file\""));
    assert!(json.contains("\"name\":\"report.pdf\""));
}

#[test]
fn a_search_result_carries_the_typed_entity_and_flat_metadata() {
    let result = SearchResult::from_entity(file("report.pdf"), 1.0);
    assert_eq!(result.entity_type, EntityType::File);
    assert_eq!(
        result.metadata.get("extension").map(String::as_str),
        Some("pdf")
    );
    assert_eq!(result.path.as_deref(), Some(r"C:\report.pdf"));
    assert!(matches!(result.entity, LocalEntity::File(_)));
}

#[test]
fn a_query_round_trips_through_the_canonical_dsl() {
    let original = "type:file ext:rs size:>1mb modified:<24h rust sort:size-desc limit:25";
    let once = query_dsl::parse(original).query;
    let twice = query_dsl::parse(&once.to_dsl()).query;
    assert_eq!(once, twice);
    assert_eq!(once.limit, 25);
}
