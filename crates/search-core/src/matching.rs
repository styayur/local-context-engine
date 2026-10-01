//! Structured predicate evaluation.
//!
//! Providers call [`passes_filters`] so that the cheap, exact part of a query
//! (`ext:rs`, `size:>10mb`, `state:running`) is applied before any candidate
//! reaches the ranker. Text matching is deliberately *not* here: it belongs to
//! the ranking crate.

use crate::clock;
use crate::entity::{drive_of, EntityType, LocalEntity};
use crate::query::{Filter, SearchQuery, TimeBound};

/// Whether `entity` satisfies every structured predicate in `query` and its
/// type restriction.
#[must_use]
pub fn passes_filters(entity: &LocalEntity, query: &SearchQuery) -> bool {
    if !query.accepts_type(entity.entity_type()) {
        return false;
    }
    query.filters.iter().all(|filter| filter.matches(entity))
}

impl Filter {
    /// Whether `entity` satisfies this predicate.
    #[must_use]
    pub fn matches(&self, entity: &LocalEntity) -> bool {
        match self {
            Filter::Extension(wanted) => {
                let wanted = wanted.trim_start_matches('.').to_ascii_lowercase();
                if wanted.is_empty() {
                    return true;
                }
                entity
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case(&wanted))
            }
            Filter::Path(needle) => entity
                .path()
                .is_some_and(|path| contains_ignore_case(path, needle)),
            Filter::Drive(letter) => {
                let wanted = letter.to_ascii_uppercase();
                matches!(
                    entity,
                    LocalEntity::File(entry) if entry.drive == Some(wanted)
                ) || matches!(
                    entity,
                    LocalEntity::Directory(entry) if entry.drive == Some(wanted)
                ) || entity
                    .path()
                    .and_then(drive_of)
                    .is_some_and(|drive| drive == wanted)
            }
            Filter::Name(needle) => contains_ignore_case(entity.name(), needle),
            Filter::Modified(bound) => bound.satisfied_by(entity.modified_ms()),
            Filter::Created(bound) => {
                let created = match entity {
                    LocalEntity::File(entry) => entry.created,
                    LocalEntity::Directory(entry) => entry.created,
                    _ => None,
                };
                bound.satisfied_by(created)
            }
            Filter::Size(size) => match entity {
                LocalEntity::File(entry) => size.op.matches(entry.size, size.bytes),
                LocalEntity::Directory(_) => size.op.matches(0, size.bytes),
                _ => false,
            },
            Filter::State(state) => state_matches(entity, state),
            Filter::Pid(pid) => match entity {
                LocalEntity::Process(entry) => entry.pid == *pid,
                LocalEntity::Window(entry) => entry.pid == *pid,
                _ => false,
            },
            Filter::User(user) => match entity {
                LocalEntity::Process(entry) => entry
                    .username
                    .as_deref()
                    .is_some_and(|name| contains_ignore_case(name, user)),
                _ => false,
            },
            Filter::Visible(visible) => match entity {
                LocalEntity::Window(entry) => entry.visible == *visible,
                _ => false,
            },
        }
    }
}

impl TimeBound {
    /// Whether `timestamp` (Unix milliseconds) satisfies this bound.
    ///
    /// A missing timestamp never satisfies a time bound, which keeps
    /// `modified:<24h` honest instead of quietly matching unknown files.
    #[must_use]
    pub fn satisfied_by(&self, timestamp: Option<i64>) -> bool {
        let Some(value) = timestamp else {
            return false;
        };
        match self {
            TimeBound::Within(duration) => value >= clock::cutoff_ms(*duration),
            TimeBound::OlderThan(duration) => value < clock::cutoff_ms(*duration),
            TimeBound::After(ms) => value >= *ms,
            TimeBound::Before(ms) => value <= *ms,
        }
    }
}

fn state_matches(entity: &LocalEntity, wanted: &str) -> bool {
    let wanted = wanted.trim().to_ascii_lowercase();
    if wanted.is_empty() {
        return true;
    }
    match entity {
        LocalEntity::Service(entry) => {
            let state = entry.state.as_str();
            let start = entry.start_type.as_str();
            wanted == state || wanted == start || state.contains(&wanted) || start.contains(&wanted)
        }
        // Every process in the live snapshot is, by definition, running.
        LocalEntity::Process(_) => matches!(wanted.as_str(), "running" | "active" | "any"),
        LocalEntity::Window(entry) => match wanted.as_str() {
            "visible" | "shown" => entry.visible,
            "hidden" => !entry.visible,
            _ => false,
        },
        _ => false,
    }
}

/// Case-insensitive `contains`.
///
/// ASCII-only inputs take an allocation-free byte-window path, which is what
/// almost every file name and process name hits.
#[must_use]
pub fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    let needle = needle.trim();
    if needle.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle.is_ascii() {
        let hay = haystack.as_bytes();
        let wanted = needle.as_bytes();
        if wanted.len() > hay.len() {
            return false;
        }
        return hay
            .windows(wanted.len())
            .any(|window| window.eq_ignore_ascii_case(wanted));
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Whether the entity type list contains anything a provider can serve.
#[must_use]
pub fn wants_any(query: &SearchQuery, types: &[EntityType]) -> bool {
    query.entity_types.is_empty() || types.iter().any(|t| query.accepts_type(*t))
}

/// Build the list of entity types a query is asking for, defaulting to all.
#[must_use]
pub fn requested_types(query: &SearchQuery) -> Vec<EntityType> {
    if query.entity_types.is_empty() {
        EntityType::ALL.to_vec()
    } else {
        query.entity_types.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{FileEntry, ProcessEntry, ServiceEntry, ServiceStartType, ServiceState};

    fn file(name: &str, path: &str, extension: Option<&str>, size: u64) -> LocalEntity {
        LocalEntity::File(FileEntry {
            path: path.to_string(),
            name: name.to_string(),
            extension: extension.map(str::to_string),
            size,
            modified: Some(clock::now_ms() - 60_000),
            created: Some(clock::now_ms() - 120_000),
            drive: path.chars().next(),
            file_id: None,
        })
    }

    #[test]
    fn extension_filter_ignores_dot_and_case() {
        let entity = file("main.RS", r"D:\src\main.RS", Some("RS"), 10);
        assert!(Filter::Extension(".rs".into()).matches(&entity));
        assert!(Filter::Extension("rs".into()).matches(&entity));
        assert!(!Filter::Extension("toml".into()).matches(&entity));
    }

    #[test]
    fn path_filter_is_case_insensitive() {
        let entity = file("readme.md", r"D:\Projects\ReadMe.md", Some("md"), 10);
        assert!(Filter::Path("projects".into()).matches(&entity));
        assert!(!Filter::Path("elsewhere".into()).matches(&entity));
    }

    #[test]
    fn size_filter_compares_bytes() {
        let entity = file("big.bin", r"C:\big.bin", Some("bin"), 20 * 1_048_576);
        assert!(Filter::Size(crate::query::SizeFilter {
            op: crate::query::SizeOp::GreaterThan,
            bytes: 10 * 1_048_576
        })
        .matches(&entity));
        assert!(!Filter::Size(crate::query::SizeFilter {
            op: crate::query::SizeOp::LessThan,
            bytes: 10 * 1_048_576
        })
        .matches(&entity));
    }

    #[test]
    fn modified_within_matches_recent_files_only() {
        let recent = file("a.txt", r"C:\a.txt", Some("txt"), 1);
        assert!(
            Filter::Modified(TimeBound::Within(std::time::Duration::from_secs(3_600)))
                .matches(&recent)
        );
        assert!(
            !Filter::Modified(TimeBound::Within(std::time::Duration::from_secs(1)))
                .matches(&recent)
        );
    }

    #[test]
    fn missing_timestamp_never_satisfies_a_time_bound() {
        let mut entity = file("a.txt", r"C:\a.txt", Some("txt"), 1);
        if let LocalEntity::File(entry) = &mut entity {
            entry.modified = None;
        }
        assert!(
            !Filter::Modified(TimeBound::Within(std::time::Duration::from_secs(3_600)))
                .matches(&entity)
        );
    }

    #[test]
    fn service_state_filter_matches_state_and_start_type() {
        let entity = LocalEntity::Service(ServiceEntry {
            name: "wuauserv".into(),
            display_name: "Windows Update".into(),
            state: ServiceState::Running,
            binary_path: None,
            start_type: ServiceStartType::Manual,
            account: None,
            pid: None,
        });
        assert!(Filter::State("running".into()).matches(&entity));
        assert!(Filter::State("manual".into()).matches(&entity));
        assert!(!Filter::State("stopped".into()).matches(&entity));
    }

    #[test]
    fn pid_filter_matches_processes() {
        let process = LocalEntity::Process(ProcessEntry {
            pid: 4321,
            name: "node.exe".into(),
            exe_path: None,
            parent_pid: None,
            memory_bytes: 0,
            start_time: None,
            username: None,
            thread_count: 1,
            session_id: None,
        });
        assert!(Filter::Pid(4321).matches(&process));
        assert!(!Filter::Pid(1).matches(&process));
    }

    #[test]
    fn drive_filter_reads_the_path() {
        let entity = file("a.txt", r"D:\a.txt", Some("txt"), 1);
        assert!(Filter::Drive('D').matches(&entity));
        assert!(Filter::Drive('d').matches(&entity));
        assert!(!Filter::Drive('C').matches(&entity));
    }

    #[test]
    fn type_restriction_is_applied_by_passes_filters() {
        let entity = file("a.txt", r"C:\a.txt", Some("txt"), 1);
        let query = SearchQuery::plain("a").with_types([EntityType::Process]);
        assert!(!passes_filters(&entity, &query));
        let query = SearchQuery::plain("a").with_types([EntityType::File]);
        assert!(passes_filters(&entity, &query));
    }

    #[test]
    fn ascii_contains_is_case_insensitive() {
        assert!(contains_ignore_case("Visual Studio Code", "studio"));
        assert!(contains_ignore_case("Project", "proj"));
        assert!(!contains_ignore_case("Project", "xyz"));
    }

    #[test]
    fn unicode_contains_is_case_insensitive() {
        assert!(contains_ignore_case("最近修改的报告", "修改"));
        assert!(contains_ignore_case("ÜBERSICHT", "übersicht"));
    }
}
