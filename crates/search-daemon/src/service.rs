//! The one search service every front end talks to.
//!
//! The CLI, the MCP server and the desktop UI all call into this type. There is
//! exactly one implementation of "run a query", one implementation of "rebuild
//! the index" and one implementation of "open this result", which is what keeps
//! the three front ends from drifting apart.

use std::sync::{Arc, RwLock};

use query_dsl::{CompiledQuery, QueryCompiler, RuleBasedCompiler};
use ranking::{HeuristicRanker, RankingWeights, UsageIndex};
use search_core::{
    EntityProvider, EntityType, LceError, LocalEntity, ProviderStats, Ranker, SearchEngine,
    SearchQuery, SearchResponse, MAX_RESULT_LIMIT,
};
use windows_apps::AppProvider;
use windows_files::{FileProvider, IndexReport};
use windows_processes::ProcessProvider;
use windows_services::ServiceProvider;
use windows_windows::WindowProvider;

use crate::actions::{self, OpenTarget};
use crate::config::{Language, Settings, Theme};
use crate::usage_store;

/// Per-query overrides that a front end can supply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchOptions {
    /// Restrict the query to these entity types.
    pub types: Vec<EntityType>,
    /// Override the configured result limit.
    pub limit: Option<usize>,
    /// Keep the compile notes for `--explain`.
    pub explain: bool,
}

/// A compiled query and the response it produced.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchOutcome {
    /// What the compiler made of the input.
    pub compiled: CompiledQuery,
    /// The ranked result set.
    pub response: SearchResponse,
}

/// Everything the index panel needs.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    /// Whether the file index can answer queries.
    pub ready: bool,
    /// The backend that produced the current index.
    pub backend: String,
    /// The backend the settings ask for.
    pub requested_backend: String,
    /// Total indexed entries.
    pub entries: usize,
    /// Indexed files.
    pub files: usize,
    /// Indexed directories.
    pub directories: usize,
    /// Approximate index memory footprint in bytes.
    pub memory_bytes: usize,
    /// Volumes covered by the index.
    pub volumes: Vec<String>,
    /// Where the cache file lives.
    pub cache_path: String,
    /// How old the persisted cache is, when there is one.
    pub cache_age_ms: Option<i64>,
    /// The last rebuild report from this session.
    pub last_report: Option<IndexReport>,
    /// Health of every provider.
    pub providers: Vec<ProviderStats>,
}

/// The shared search service.
#[derive(Debug)]
pub struct SearchService {
    files: Arc<FileProvider>,
    apps: Arc<AppProvider>,
    processes: Arc<ProcessProvider>,
    services: Arc<ServiceProvider>,
    windows: Arc<WindowProvider>,
    compiler: RuleBasedCompiler,
    usage: Arc<RwLock<UsageIndex>>,
    settings: RwLock<Settings>,
}

impl SearchService {
    /// Build a service from explicit settings.
    #[must_use]
    pub fn new(mut settings: Settings, usage: UsageIndex) -> Self {
        settings.normalise();
        let files = Arc::new(FileProvider::new(
            settings.index_config(),
            settings.index_backend,
            "default",
        ));
        Self {
            files,
            apps: Arc::new(AppProvider::new()),
            processes: Arc::new(ProcessProvider::new()),
            services: Arc::new(ServiceProvider::new()),
            windows: Arc::new(WindowProvider::new()),
            compiler: RuleBasedCompiler::new(),
            usage: Arc::new(RwLock::new(usage)),
            settings: RwLock::new(settings),
        }
    }

    /// Build a service from the settings and usage history on disk.
    #[must_use]
    pub fn bootstrap() -> Self {
        Self::new(Settings::load(), usage_store::load())
    }

    /// The filesystem provider, for index management.
    #[must_use]
    pub fn files(&self) -> &Arc<FileProvider> {
        &self.files
    }

    /// A snapshot of the current settings.
    #[must_use]
    pub fn settings(&self) -> Settings {
        self.settings
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Replace the settings and persist them.
    pub fn update_settings(&self, mut settings: Settings) -> Result<Settings, LceError> {
        settings.normalise();
        {
            let Ok(mut guard) = self.settings.write() else {
                return Err(LceError::Io {
                    action: "updating settings".into(),
                    detail: "settings lock was poisoned".into(),
                });
            };
            *guard = settings.clone();
        }
        self.files.set_config(settings.index_config());
        settings.save()?;
        Ok(settings)
    }

    /// The resolved locale the UI should use.
    #[must_use]
    pub fn locale(&self) -> &'static str {
        self.settings
            .read()
            .map(|guard| guard.language.resolve())
            .unwrap_or("en-US")
    }

    /// The configured theme.
    #[must_use]
    pub fn theme(&self) -> Theme {
        self.settings
            .read()
            .map(|guard| guard.theme)
            .unwrap_or_default()
    }

    /// The configured language.
    #[must_use]
    pub fn language(&self) -> Language {
        self.settings
            .read()
            .map(|guard| guard.language)
            .unwrap_or_default()
    }

    /// Build an engine over the current provider set.
    ///
    /// Constructing one is two `Arc`s and a handful of floats, so the service
    /// rebuilds it per query rather than caching a mutable one. That is what
    /// makes a settings change take effect immediately, with no restart.
    #[must_use]
    pub fn engine(&self) -> SearchEngine {
        let settings = self.settings();
        let mut weights = RankingWeights::default();
        if !settings.fuzzy_matching {
            weights.fuzzy_match = 0.0;
        }
        let mut ranker = HeuristicRanker::new(weights);
        if settings.usage_ranking {
            ranker = ranker.with_usage(Arc::clone(&self.usage));
        }

        let providers: Vec<Arc<dyn search_core::EntityProvider>> = vec![
            Arc::clone(&self.apps) as Arc<dyn search_core::EntityProvider>,
            Arc::clone(&self.processes) as Arc<dyn search_core::EntityProvider>,
            Arc::clone(&self.files) as Arc<dyn search_core::EntityProvider>,
            Arc::clone(&self.services) as Arc<dyn search_core::EntityProvider>,
            Arc::clone(&self.windows) as Arc<dyn search_core::EntityProvider>,
        ];
        SearchEngine::new(providers, Arc::new(ranker) as Arc<dyn Ranker>)
    }

    /// Compile an input without running a search.
    ///
    /// This is the whole Query Compiler surface on its own, which is what MCP
    /// clients and `--explain` want when they are inspecting an interpretation
    /// rather than asking for results.
    #[must_use]
    pub fn compile(&self, input: &str) -> CompiledQuery {
        self.compiler.compile_detailed(input)
    }

    /// Compile and run a query.
    #[must_use]
    pub fn search(&self, input: &str, options: &SearchOptions) -> SearchResponse {
        self.search_detailed(input, options).response
    }

    /// Compile and run a query, keeping the compile provenance.
    #[must_use]
    pub fn search_detailed(&self, input: &str, options: &SearchOptions) -> SearchOutcome {
        let settings = self.settings();
        let mut compiled = self.compiler.compile_detailed(input);

        if !options.types.is_empty() {
            compiled.query.entity_types = options.types.clone();
        }
        let limit = options
            .limit
            .unwrap_or(settings.result_limit)
            .clamp(1, MAX_RESULT_LIMIT);
        compiled.query.limit = limit;

        let mut response = self.engine().search(input, &compiled.query);
        response.compiled = compiled.to_dsl();
        if matches!(compiled.source, query_dsl::CompileSource::Dsl) {
            // The DSL is already structured; nothing else to explain here.
        }
        SearchOutcome { compiled, response }
    }

    /// Run a query through the privileged index service, when it is running.
    ///
    /// Returns `Ok(None)` when the service is not installed or not reachable,
    /// which callers treat as "use the local provider" rather than a failure.
    /// The fallback is what keeps the product usable on a machine where nobody
    /// wants to install a service.
    pub fn search_via_service(
        &self,
        input: &str,
        options: &SearchOptions,
    ) -> Result<Option<SearchResponse>, LceError> {
        let settings = self.settings();
        let limit = options
            .limit
            .unwrap_or(settings.result_limit)
            .clamp(1, MAX_RESULT_LIMIT);
        crate::service_client::search(input, &options.types, limit)
    }

    /// The index service's status, as the front ends report it.
    #[must_use]
    pub fn service_status(&self) -> crate::service_client::ServiceStatus {
        crate::service_client::status()
    }

    /// Run a query that is already structured.    /// Run a query that is already structured.
    #[must_use]
    pub fn search_query(&self, query: &SearchQuery) -> SearchResponse {
        self.engine().search(&query.to_dsl(), query)
    }

    /// Health of every provider.
    #[must_use]
    pub fn provider_stats(&self) -> Vec<ProviderStats> {
        self.engine().provider_stats()
    }

    /// Everything the index panel needs.
    #[must_use]
    pub fn index_status(&self) -> IndexStatus {
        let stats = self.files.stats();
        let entry = |key: &str| -> usize {
            stats
                .detail
                .get(key)
                .and_then(|value| value.parse().ok())
                .unwrap_or(0)
        };

        IndexStatus {
            ready: stats.ready,
            backend: self.files.active_backend(),
            requested_backend: self.files.requested_backend().as_str().to_string(),
            entries: self.files.indexed_entries(),
            files: entry("files"),
            directories: entry("directories"),
            memory_bytes: self.files.memory_bytes(),
            volumes: stats
                .detail
                .get("volumes")
                .map(|value| {
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|part| !part.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            cache_path: self.files.cache_path().to_string_lossy().to_string(),
            cache_age_ms: windows_files::provider::cache_age_ms(self.files.cache_path()),
            last_report: self.files.last_report(),
            providers: self.provider_stats(),
        }
    }

    /// Rebuild the file index.
    pub fn rebuild_index(&self) -> Result<IndexReport, LceError> {
        self.files.rebuild()
    }

    /// Apply incremental index updates, if the backend has any.
    pub fn update_index(&self) -> Result<Option<windows_files::UpdateReport>, LceError> {
        self.files.update()
    }

    /// Re-read everything that caches data.
    pub fn refresh(&self) -> (usize, Vec<LceError>) {
        self.engine().refresh_all()
    }

    /// Remember that a result was chosen for a query, and persist it.
    pub fn record_selection(&self, query: &str, entity_id: &str) -> Result<(), LceError> {
        {
            let Ok(mut guard) = self.usage.write() else {
                return Err(LceError::Io {
                    action: "recording a selection".into(),
                    detail: "usage lock was poisoned".into(),
                });
            };
            guard.record(query, entity_id);
        }
        self.save_usage()
    }

    /// Forget the usage history.
    pub fn reset_usage(&self) -> Result<(), LceError> {
        if let Ok(mut guard) = self.usage.write() {
            guard.clear();
        }
        usage_store::reset()
    }

    /// Persist the usage history.
    pub fn save_usage(&self) -> Result<(), LceError> {
        let Ok(guard) = self.usage.read() else {
            return Ok(());
        };
        usage_store::save(&guard)
    }

    /// A copy of the usage history, most recent first.
    #[must_use]
    pub fn recent_selections(&self, limit: usize) -> Vec<ranking::UsageEntry> {
        self.usage
            .read()
            .map(|guard| guard.recent(limit))
            .unwrap_or_default()
    }

    /// Open a result with its registered handler.
    pub fn open(&self, entity: &LocalEntity) -> Result<(), LceError> {
        match entity {
            LocalEntity::Application(entry) => {
                actions::launch(&entry.target, entry.arguments.as_deref())
            }
            LocalEntity::Window(entry) => Err(LceError::Platform(
                search_core::PlatformError::Unsupported {
                    feature: format!("activating window {} by handle", entry.hwnd),
                },
            )),
            other => actions::open(other, OpenTarget::Default),
        }
    }

    /// Reveal a result in the file manager.
    pub fn reveal(&self, entity: &LocalEntity) -> Result<(), LceError> {
        match entity.path() {
            Some(path) => actions::reveal(path),
            None => Err(LceError::Platform(search_core::PlatformError::NotFound {
                resource: format!("a filesystem location for `{}`", entity.name()),
            })),
        }
    }

    /// End a process. `confirmed` must be true.
    pub fn terminate_process(&self, pid: u32, confirmed: bool) -> Result<(), LceError> {
        actions::terminate_process(pid, confirmed)
    }

    /// The ranking weights currently in effect.
    #[must_use]
    pub fn ranking_weights(&self) -> RankingWeights {
        let mut weights = RankingWeights::default();
        if !self.settings().fuzzy_matching {
            weights.fuzzy_match = 0.0;
        }
        weights
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use search_core::Filter;

    fn service() -> SearchService {
        SearchService::new(Settings::default(), UsageIndex::default())
    }

    #[test]
    fn a_plain_query_returns_a_response() {
        let service = service();
        let response = service.search("vscode", &SearchOptions::default());
        assert_eq!(response.query, "vscode");
        assert_eq!(response.compiled, "vscode");
        assert!(response.elapsed_ms >= 0.0);
    }

    #[test]
    fn the_dsl_is_compiled_into_the_response() {
        let service = service();
        let outcome = service.search_detailed(
            "type:process python",
            &SearchOptions {
                explain: true,
                ..SearchOptions::default()
            },
        );
        assert_eq!(outcome.compiled.to_dsl(), "type:process python");
        assert_eq!(
            outcome.compiled.query.entity_types,
            vec![EntityType::Process]
        );
    }

    #[test]
    fn natural_language_is_compiled() {
        let service = service();
        let outcome = service.search_detailed("最近的 pdf", &SearchOptions::default());
        assert_eq!(
            outcome.compiled.to_dsl(),
            "type:file ext:pdf sort:modified-desc"
        );
    }

    #[test]
    fn search_options_override_the_type_filter() {
        let service = service();
        let response = service.search(
            "x",
            &SearchOptions {
                types: vec![EntityType::Service],
                ..SearchOptions::default()
            },
        );
        assert!(response
            .results
            .iter()
            .all(|result| result.entity_type == EntityType::Service));
    }

    #[test]
    fn the_result_limit_is_honoured() {
        let service = service();
        let response = service.search(
            "",
            &SearchOptions {
                limit: Some(3),
                ..SearchOptions::default()
            },
        );
        assert!(response.results.len() <= 3);
    }

    #[test]
    fn a_process_query_finds_the_current_process() {
        let service = service();
        let query = SearchQuery::plain("")
            .with_filter(Filter::Pid(std::process::id()))
            .with_types([EntityType::Process]);
        let response = service.search_query(&query);
        assert!(
            response
                .results
                .iter()
                .any(|result| result.metadata.get("pid").map(String::as_str)
                    == Some(std::process::id().to_string().as_str())),
            "expected the test process in {:?}",
            response.results
        );
    }

    #[test]
    fn index_status_reports_a_shape_even_before_the_first_build() {
        let service = service();
        let status = service.index_status();
        assert_eq!(status.requested_backend, "auto");
        assert!(!status.providers.is_empty());
        assert!(status.cache_path.ends_with(".lce-index"));
    }

    #[test]
    fn provider_stats_cover_every_domain() {
        let service = service();
        let names: Vec<String> = service
            .provider_stats()
            .into_iter()
            .map(|stats| stats.name)
            .collect();
        for expected in ["files", "apps", "processes", "services", "windows"] {
            assert!(names.contains(&expected.to_string()), "{names:?}");
        }
    }

    #[test]
    fn settings_changes_take_effect_without_a_restart() {
        let service = service();
        assert_eq!(service.locale(), Language::System.resolve());
        let mut settings = service.settings();
        settings.language = Language::ZhCn;
        service.update_settings(settings).unwrap();
        assert_eq!(service.locale(), "zh-CN");

        let mut settings = service.settings();
        settings.language = Language::System;
        settings.theme = Theme::Dark;
        settings.result_limit = 20;
        service.update_settings(settings).unwrap();
        assert_eq!(service.settings().result_limit, 20);
        assert_eq!(service.theme(), Theme::Dark);
    }

    #[test]
    fn disabling_fuzzy_matching_changes_the_weights() {
        let service = service();
        let mut settings = service.settings();
        settings.fuzzy_matching = false;
        service.update_settings(settings).unwrap();
        assert_eq!(service.ranking_weights().fuzzy_match, 0.0);

        let mut settings = service.settings();
        settings.fuzzy_matching = true;
        service.update_settings(settings).unwrap();
        assert!(service.ranking_weights().fuzzy_match > 0.0);
    }

    #[test]
    fn usage_is_recorded_and_boosted() {
        let service = SearchService::new(Settings::default(), UsageIndex::default());
        let target = "app:c:\\definitely-not-real.exe";
        service.record_selection("q", target).unwrap();
        assert_eq!(service.recent_selections(5).len(), 1);
        service.reset_usage().unwrap();
        assert!(service.recent_selections(5).is_empty());
    }

    #[test]
    fn terminating_a_process_requires_confirmation() {
        let service = service();
        assert!(service
            .terminate_process(std::process::id(), false)
            .is_err());
    }

    #[test]
    fn a_service_result_can_be_revealed_by_path() {
        let service = service();
        let entity = LocalEntity::Service(search_core::ServiceEntry {
            name: "demo".into(),
            display_name: "Demo".into(),
            state: search_core::ServiceState::Running,
            binary_path: None,
            start_type: search_core::ServiceStartType::Manual,
            account: None,
            pid: None,
        });
        assert!(service.reveal(&entity).is_err());
    }
}
