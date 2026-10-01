//! `localsearch` — the command line front end.
//!
//! The CLI exists for three audiences: people who live in a terminal, scripts
//! and CI that need machine-readable output, and AI agents that want a cheap
//! deterministic way to ask the local machine a question.
//!
//! Exit codes are part of the contract:
//!
//! | code | meaning                                   |
//! |------|-------------------------------------------|
//! | 0    | at least one result was returned          |
//! | 1    | the query was valid and matched nothing   |
//! | 2    | the command could not be completed        |

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::{ArgAction, Parser};
use search_core::{clock, EntityType, LceError, LocalEntity, ProviderTiming, SearchResponse, Sort};
use search_daemon::{IndexBackend, SearchOptions, SearchService, Settings};

mod render;

use render::{print_index_status, print_report, Renderer};

/// Natural language or Local Search DSL, compiled locally and searched by the
/// Rust core. Nothing leaves your machine.
#[derive(Debug, Parser)]
#[command(
    name = "localsearch",
    version,
    about = "Millisecond, offline-first search over your local system",
    long_about = "Local Context Engine compiles natural language or the Local Search DSL into a \
structured query, then answers it from a deterministic Rust search core. \
No network, no telemetry, no model on the hot path.",
    after_help = "EXAMPLES:\n  \
localsearch vscode\n  \
localsearch \"type:file ext:rs modified:<24h\"\n  \
localsearch \"正在运行的 python\"\n  \
localsearch --type process node\n  \
localsearch --json vscode\n  \
localsearch --index-status\n  \
localsearch --rebuild-index"
)]
struct Cli {
    /// Search terms: plain text, natural language, or Local Search DSL.
    #[arg(value_name = "QUERY", trailing_var_arg = true)]
    query: Vec<String>,

    /// Restrict to one or more entity types (repeatable, comma separated).
    #[arg(short = 't', long = "type", value_name = "TYPE", value_delimiter = ',')]
    types: Vec<String>,

    /// Maximum number of results.
    #[arg(short = 'n', long = "limit", value_name = "N")]
    limit: Option<usize>,

    /// Print the results as JSON.
    #[arg(short = 'j', long = "json")]
    json: bool,

    /// Show how the input was compiled and how each result was scored.
    #[arg(short = 'e', long = "explain")]
    explain: bool,

    /// Suppress the header and footer, printing only the results.
    #[arg(short = 'q', long = "quiet")]
    quiet: bool,

    /// Never emit colour, even on a terminal.
    #[arg(long = "no-color")]
    no_color: bool,

    /// Sort the results explicitly, for example `size-desc` or `modified-desc`.
    #[arg(short = 's', long = "sort", value_name = "ORDER")]
    sort: Option<String>,

    /// Show index health, backends and entry counts.
    #[arg(long = "index-status", action = ArgAction::SetTrue)]
    index_status: bool,

    /// Rebuild the file index from scratch.
    #[arg(long = "rebuild-index", action = ArgAction::SetTrue)]
    rebuild_index: bool,

    /// Apply incremental index updates from the change journal.
    #[arg(long = "update-index", action = ArgAction::SetTrue)]
    update_index: bool,

    /// Show provider health.
    #[arg(long = "providers", action = ArgAction::SetTrue)]
    providers: bool,

    /// Choose the index backend.
    #[arg(long = "backend", value_name = "auto|scan|mft-usn")]
    backend: Option<String>,

    /// Override the interface language for this run.
    #[arg(long = "lang", value_name = "system|zh-CN|en-US")]
    lang: Option<String>,

    /// List the results you have chosen before.
    #[arg(long = "list-usage", action = ArgAction::SetTrue)]
    list_usage: bool,

    /// Forget the usage history.
    #[arg(long = "reset-usage", action = ArgAction::SetTrue)]
    reset_usage: bool,

    /// Increase log verbosity (repeatable).
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count)]
    verbose: u8,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match run(&cli) {
        Ok(code) => code,
        Err(error) => {
            report_error(&cli, &error);
            ExitCode::from(2)
        }
    }
}

fn run(cli: &Cli) -> Result<ExitCode, LceError> {
    let mut settings = Settings::load();
    if let Some(backend) = cli.backend.as_deref() {
        settings.index_backend = IndexBackend::parse(backend).ok_or_else(|| {
            LceError::Search(search_core::SearchError::InvalidQuery {
                reason: format!("unknown backend `{backend}`; expected auto, scan or mft-usn"),
            })
        })?;
    }
    if let Some(lang) = cli.lang.as_deref() {
        settings.language = search_daemon::Language::parse(lang).ok_or_else(|| {
            LceError::Search(search_core::SearchError::InvalidQuery {
                reason: format!("unknown language `{lang}`; expected system, zh-CN or en-US"),
            })
        })?;
    }
    if cli.limit.is_some() {
        settings.result_limit = cli.limit.unwrap_or(settings.result_limit);
        settings.normalise();
    }

    let service = SearchService::new(settings, search_daemon::UsageIndex::default());
    let renderer = Renderer::new(cli.json, cli.no_color, cli.quiet);

    if cli.reset_usage {
        service.reset_usage()?;
        if !cli.quiet {
            println!("usage history cleared");
        }
        return Ok(ExitCode::SUCCESS);
    }

    if cli.list_usage {
        let entries = service.recent_selections(20);
        return renderer.print_usage(&entries);
    }

    if cli.index_status {
        return print_index_status(&service, &renderer);
    }

    if cli.providers {
        return renderer.print_providers(&service.provider_stats());
    }

    if cli.rebuild_index {
        return print_report(&service, &renderer, true);
    }

    if cli.update_index {
        return print_report(&service, &renderer, false);
    }

    let input = cli.query.join(" ");
    if input.trim().is_empty() {
        return Err(LceError::Search(search_core::SearchError::InvalidQuery {
            reason: "no query given; try `localsearch vscode` or `localsearch --help`".into(),
        }));
    }

    let mut options = SearchOptions {
        types: parse_types(&cli.types)?,
        limit: cli.limit,
        explain: cli.explain,
    };
    if options.limit.is_some() {
        options.limit = options
            .limit
            .map(|limit| limit.clamp(1, search_core::MAX_RESULT_LIMIT));
    }

    let mut outcome = service.search_detailed(&input, &options);
    if let Some(sort) = cli.sort.as_deref() {
        let sort = Sort::parse(sort).ok_or_else(|| {
            LceError::Search(search_core::SearchError::InvalidQuery {
                reason: format!("unknown sort order `{sort}`; try `size-desc` or `modified-desc`"),
            })
        })?;
        let mut query = outcome.compiled.query.clone();
        query.sort = Some(sort);
        let mut response = service.search_query(&query);
        response.query = outcome.response.query.clone();
        outcome.response = response;
    }

    if cli.explain {
        renderer.print_explanation(&outcome.compiled);
    }
    renderer.print_response(&outcome.response)?;

    Ok(if outcome.response.results.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn parse_types(values: &[String]) -> Result<Vec<EntityType>, LceError> {
    let mut types = Vec::new();
    for value in values {
        for part in value.split([',', '|']) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let entity_type = EntityType::parse(part).ok_or_else(|| {
                LceError::Search(search_core::SearchError::InvalidQuery {
                    reason: format!(
                        "unknown entity type `{part}`; expected file, directory, process, app, service or window"
                    ),
                })
            })?;
            if !types.contains(&entity_type) {
                types.push(entity_type);
            }
        }
    }
    Ok(types)
}

fn init_tracing(verbosity: u8) {
    let level = match verbosity {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("LCE_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        // Diagnostics must never pollute stdout: `--json` writes a single
        // document there and scripts parse it.
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

fn report_error(cli: &Cli, error: &LceError) {
    if cli.json {
        let payload = serde_json::json!({
            "error": {
                "code": error.code(),
                "message": error.to_string(),
                "hint": error.hint(),
            }
        });
        eprintln!("{payload}");
        return;
    }
    eprintln!("error: {}", error.hint());
    eprintln!("       code: {} ({})", error.code(), error);
    if !error.is_recoverable() {
        eprintln!("       run with -v for the developer log");
    }
}

/// Whether the terminal wants colour.
#[must_use]
pub fn wants_colour(no_color: bool) -> bool {
    !no_color && std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

/// Format the relative age of a timestamp for the result list.
#[must_use]
pub fn format_age(ms: Option<i64>) -> String {
    let Some(ms) = ms else {
        return String::new();
    };
    let age = (clock::now_ms() - ms).max(0);
    let minutes = age / 60_000;
    if minutes < 1 {
        "just now".into()
    } else if minutes < 60 {
        format!("{minutes}m ago")
    } else if minutes < 60 * 24 {
        format!("{}h ago", minutes / 60)
    } else if minutes < 60 * 24 * 30 {
        format!("{}d ago", minutes / (60 * 24))
    } else {
        clock::format_unix_ms(ms)
    }
}

/// A one-line summary of what the query hit, per provider.
#[must_use]
pub fn summarise_timings(timings: &[ProviderTiming]) -> String {
    if timings.is_empty() {
        return String::new();
    }
    timings
        .iter()
        .filter(|timing| timing.candidates > 0)
        .map(|timing| {
            format!(
                "{} {} in {:.1}ms",
                timing.candidates, timing.provider, timing.elapsed_ms
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The entity types a result set contains, in display order.
#[must_use]
pub fn types_present(response: &SearchResponse) -> Vec<EntityType> {
    let mut types: Vec<EntityType> = Vec::new();
    for result in &response.results {
        if !types.contains(&result.entity_type) {
            types.push(result.entity_type);
        }
    }
    types
}

/// A short label describing an entity's actionable path.
#[must_use]
pub fn subject_of(entity: &LocalEntity) -> String {
    entity
        .path()
        .map(str::to_string)
        .unwrap_or_else(|| entity.subtitle())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn type_parsing_accepts_aliases_and_lists() {
        let types = parse_types(&["file,process".into(), "app".into()]).unwrap();
        assert_eq!(
            types,
            vec![
                EntityType::File,
                EntityType::Process,
                EntityType::Application
            ]
        );
    }

    #[test]
    fn type_parsing_rejects_nonsense() {
        assert!(parse_types(&["nonsense".into()]).is_err());
    }

    #[test]
    fn age_formatting_is_human_readable() {
        let now = clock::now_ms();
        assert_eq!(format_age(Some(now)), "just now");
        assert_eq!(format_age(Some(now - 5 * 60_000)), "5m ago");
        assert_eq!(format_age(Some(now - 3 * 3_600_000)), "3h ago");
        assert_eq!(format_age(None), "");
    }

    #[test]
    fn timings_are_summarised_compactly() {
        let timings = vec![
            ProviderTiming {
                provider: "files".into(),
                candidates: 120,
                elapsed_ms: 0.4,
            },
            ProviderTiming {
                provider: "apps".into(),
                candidates: 0,
                elapsed_ms: 0.1,
            },
        ];
        let summary = summarise_timings(&timings);
        assert!(summary.contains("120 files"));
        assert!(!summary.contains("apps"));
    }

    #[test]
    fn colour_is_disabled_when_asked() {
        assert!(!wants_colour(true));
    }

    #[test]
    fn an_app_entity_subject_is_its_target() {
        let entity = LocalEntity::Application(search_core::AppEntry {
            name: "Code".into(),
            target: r"C:\Code.exe".into(),
            arguments: None,
            working_dir: None,
            source: search_core::AppSource::AppPaths,
        });
        assert_eq!(subject_of(&entity), r"C:\Code.exe");
    }
}
