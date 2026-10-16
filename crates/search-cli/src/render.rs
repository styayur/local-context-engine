//! Terminal rendering.
//!
//! Human output and `--json` output come from the same response, so the two can
//! never disagree about what was found.

use std::process::ExitCode;

use search_core::{
    clock, human_size, LceError, LocalEntity, MatchField, ProviderStats, SearchResponse,
};
use search_daemon::{CompiledQuery, IndexStatus, SearchService};

use crate::{format_age, summarise_timings, wants_colour};

const BOLD: &str = "\u{1b}[1m";
const DIM: &str = "\u{1b}[2m";
const HL: &str = "\u{1b}[1;36m";
const RESET: &str = "\u{1b}[0m";

/// Formats one command's output.
#[derive(Debug, Clone, Copy)]
pub struct Renderer {
    json: bool,
    colour: bool,
    quiet: bool,
}

impl Renderer {
    /// Build a renderer from the command line flags.
    #[must_use]
    pub fn new(json: bool, no_color: bool, quiet: bool) -> Self {
        Self {
            json,
            colour: wants_colour(no_color),
            quiet,
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.colour {
            format!("{code}{text}{RESET}")
        } else {
            text.to_string()
        }
    }

    /// Print the compile explanation.
    pub fn print_explanation(&self, compiled: &CompiledQuery) {
        if self.json {
            return;
        }
        println!(
            "{}",
            self.paint(
                DIM,
                &format!("compiled via {} -> {}", compiled.source, compiled.to_dsl())
            )
        );
        for note in &compiled.notes {
            println!("{}", self.paint(DIM, &format!("  · {note}")));
        }
    }

    /// Print the index query plan, as `--explain` promises.
    ///
    /// The numbers come from the same planner the search itself used, so this
    /// is a report rather than a re-derivation.
    pub fn print_plan(&self, info: &search_daemon::windows_files_plan::QueryPlanInfo) {
        if self.json {
            return;
        }
        println!(
            "{}",
            self.paint(
                DIM,
                &format!(
                    "plan: {}   initial candidates: {}   after filters: {}   ranked: {}",
                    info.plan.as_str(),
                    info.initial_candidates,
                    info.after_filters,
                    info.verified
                )
            )
        );
        if !info.sources.is_empty() {
            println!(
                "{}",
                self.paint(DIM, &format!("  sources: {}", info.describe_sources()))
            );
        }
        if let Some(note) = &info.note {
            println!("{}", self.paint(DIM, &format!("  note: {note}")));
        }
    }

    /// Print a search response.
    pub fn print_response(&self, response: &SearchResponse) -> Result<ExitCode, LceError> {
        if self.json {
            let payload = serde_json::json!({
                "query": response.query,
                "compiled": response.compiled,
                "elapsed_ms": response.elapsed_ms,
                "total": response.total,
                "truncated": response.truncated,
                "warnings": response.warnings,
                "timings": response.timings,
                "results": response.results,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".into())
            );
            return Ok(if response.results.is_empty() {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            });
        }

        if !self.quiet {
            println!(
                "{}",
                self.paint(
                    DIM,
                    &format!(
                        "{}  ·  {} result(s)  ·  {:.1} ms",
                        response.compiled, response.total, response.elapsed_ms
                    )
                )
            );
        }

        for (index, result) in response.results.iter().enumerate() {
            let highlights = if result.matched_field == MatchField::Name {
                &result.match_ranges[..]
            } else {
                &[][..]
            };
            let name = highlight(&result.display_name, highlights, self.colour);
            println!(
                "{:>3}. {}  {}  {}",
                index + 1,
                name,
                self.paint(DIM, &format!("[{}]", result.entity_type.label())),
                self.paint(DIM, &format!("{:.1}", result.score))
            );
            let subtitle = subtitle_for(&result.entity, result);
            if !subtitle.is_empty() && !self.quiet {
                println!("     {}", self.paint(DIM, &subtitle));
            }
        }

        if !self.quiet && response.results.is_empty() {
            println!("{}", self.paint(DIM, "no results"));
        }

        if !self.quiet {
            let timings = summarise_timings(&response.timings);
            if !timings.is_empty() {
                println!("{}", self.paint(DIM, &timings));
            }
            if response.truncated {
                println!(
                    "{}",
                    self.paint(DIM, "more results exist; raise --limit to see them")
                );
            }
        }

        for warning in &response.warnings {
            eprintln!("warning: {warning}");
        }

        Ok(if response.results.is_empty() {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        })
    }

    /// Print provider health.
    pub fn print_providers(&self, stats: &[ProviderStats]) -> Result<ExitCode, LceError> {
        if self.json {
            println!(
                "{}",
                serde_json::to_string_pretty(stats).unwrap_or_else(|_| "[]".into())
            );
            return Ok(ExitCode::SUCCESS);
        }
        println!(
            "{:<12} {:<8} {:>10}  DETAIL",
            "PROVIDER", "SCOPE", "ENTITIES"
        );
        for stat in stats {
            let detail = stat
                .detail
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(" ");
            println!(
                "{:<12} {:<8} {:>10}  {}",
                stat.name,
                stat.scope.as_str(),
                stat.entity_count
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "-".into()),
                detail
            );
        }
        Ok(ExitCode::SUCCESS)
    }

    /// Print remembered selections.
    pub fn print_usage(&self, entries: &[search_daemon::UsageEntry]) -> Result<ExitCode, LceError> {
        if self.json {
            println!(
                "{}",
                serde_json::to_string_pretty(entries).unwrap_or_else(|_| "[]".into())
            );
            return Ok(ExitCode::SUCCESS);
        }
        if entries.is_empty() {
            println!("no usage history recorded yet");
            return Ok(ExitCode::SUCCESS);
        }
        for entry in entries {
            println!(
                "{:>4}×  {}  {}",
                entry.selection_count,
                entry.entity_id,
                self.paint(DIM, &format_age(Some(entry.last_selected_ms)))
            );
        }
        Ok(ExitCode::SUCCESS)
    }

    /// Print the index status.
    pub fn print_index(&self, status: &IndexStatus) -> Result<ExitCode, LceError> {
        if self.json {
            println!(
                "{}",
                serde_json::to_string_pretty(status).unwrap_or_else(|_| "{}".into())
            );
            return Ok(ExitCode::SUCCESS);
        }

        println!("{}", self.paint(BOLD, "Local Context Engine — index"));
        println!(
            "  state              {}",
            if status.ready { "ready" } else { "not built" }
        );
        println!(
            "  backend            {} (requested {})",
            status.backend, status.requested_backend
        );
        println!("  entries            {}", status.entries);
        println!("  files              {}", status.files);
        println!("  directories        {}", status.directories);
        println!(
            "  memory             {}",
            human_size(status.memory_bytes as u64)
        );
        println!(
            "  volumes            {}",
            if status.volumes.is_empty() {
                "-".into()
            } else {
                status.volumes.join(", ")
            }
        );
        println!("  cache              {}", status.cache_path);
        if let Some(age) = status.cache_age_ms {
            println!(
                "  cache age          {}",
                format_age(Some(clock::now_ms() - age))
            );
        }
        if let Some(report) = &status.last_report {
            println!(
                "  last rebuild       {} entries in {:.1} ms via {}",
                report.entries, report.elapsed_ms, report.backend
            );
        }
        println!();
        for provider in &status.providers {
            let marker = if provider.ready { "ok " } else { "off" };
            println!(
                "  [{marker}] {:<10} {:<8} {}",
                provider.name,
                provider.scope.as_str(),
                provider
                    .entity_count
                    .map(|count| format!("{count} entities"))
                    .unwrap_or_else(|| "-".into())
            );
            for warning in &provider.warnings {
                println!("        {}", self.paint(DIM, warning));
            }
        }
        Ok(ExitCode::SUCCESS)
    }

    /// Print the result of an index rebuild or update.
    pub fn print_report(&self, value: &serde_json::Value) -> Result<ExitCode, LceError> {
        if self.json {
            println!(
                "{}",
                serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".into())
            );
        } else {
            println!("{value}");
        }
        Ok(ExitCode::SUCCESS)
    }
}

/// Render a name with its matched ranges highlighted.
#[must_use]
pub fn highlight(text: &str, ranges: &[search_core::MatchRange], colour: bool) -> String {
    if ranges.is_empty() || !colour {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + ranges.len() * 8);
    let mut cursor = 0usize;
    for range in ranges {
        let start = range.start.min(chars.len());
        let end = range.end.min(chars.len());
        if start > cursor {
            out.extend(&chars[cursor..start]);
        }
        if end > start {
            out.push_str(HL);
            out.extend(&chars[start..end]);
            out.push_str(RESET);
        }
        cursor = end.max(cursor);
    }
    if cursor < chars.len() {
        out.extend(&chars[cursor..]);
    }
    out
}

fn subtitle_for(entity: &LocalEntity, result: &search_core::SearchResult) -> String {
    let mut parts: Vec<String> = Vec::new();
    match entity {
        LocalEntity::File(entry) => {
            parts.push(human_size(entry.size));
            parts.push(format_age(entry.modified));
            parts.push(entry.path.clone());
        }
        LocalEntity::Directory(entry) => {
            parts.push(format_age(entry.modified));
            parts.push(entry.path.clone());
        }
        _ => parts.push(result.subtitle.clone()),
    }
    parts.retain(|part| !part.is_empty());
    parts.join(" · ")
}

/// Handle `--index-status`.
pub fn print_index_status(
    service: &SearchService,
    renderer: &Renderer,
) -> Result<ExitCode, LceError> {
    let status = service.index_status();
    renderer.print_index(&status)
}

/// Handle `--rebuild-index` and `--update-index`.
pub fn print_report(
    service: &SearchService,
    renderer: &Renderer,
    rebuild: bool,
) -> Result<ExitCode, LceError> {
    if rebuild {
        let report = service.rebuild_index()?;
        let payload = serde_json::json!({
            "action": "rebuild",
            "backend": report.backend,
            "volumes": report.volumes,
            "entries": report.entries,
            "directories": report.directories,
            "truncated": report.truncated,
            "elapsed_ms": report.elapsed_ms,
            "warnings": report.warnings,
        });
        return renderer.print_report(&payload);
    }

    match service.update_index()? {
        Some(report) => {
            let payload = serde_json::json!({
                "action": "update",
                "examined": report.examined,
                "created": report.applied.created,
                "deleted": report.applied.deleted,
                "renamed": report.applied.renamed,
                "metadata": report.applied.metadata,
                "skipped": report.applied.skipped,
                "volumes": report.volumes,
                "requires_rebuild": report.requires_rebuild,
            });
            renderer.print_report(&payload)
        }
        None => {
            let payload = serde_json::json!({
                "action": "update",
                "applied": false,
                "reason": "the active backend has no incremental source; rebuild to refresh the index",
            });
            renderer.print_report(&payload)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use search_core::MatchRange;

    #[test]
    fn highlighting_is_a_no_op_without_colour() {
        let ranges = vec![MatchRange { start: 0, end: 4 }];
        assert_eq!(highlight("code.exe", &ranges, false), "code.exe");
    }

    #[test]
    fn highlighting_wraps_only_the_matched_span() {
        let ranges = vec![MatchRange { start: 0, end: 4 }];
        let rendered = highlight("code.exe", &ranges, true);
        assert!(rendered.contains("code"));
        assert!(rendered.contains("exe"));
        assert!(rendered.contains("\u{1b}[1;36m"));
    }

    #[test]
    fn highlighting_handles_unicode_ranges() {
        let ranges = vec![MatchRange { start: 0, end: 2 }];
        let rendered = highlight("年度报告.pdf", &ranges, true);
        // Byte offsets would slice this string in the wrong place; the range is
        // in characters, so exactly "年度" is wrapped.
        assert_eq!(rendered, "\u{1b}[1;36m年度\u{1b}[0m报告.pdf");
    }

    #[test]
    fn highlighting_is_robust_to_out_of_range_spans() {
        let ranges = vec![MatchRange { start: 3, end: 99 }];
        let rendered = highlight("ab", &ranges, true);
        assert!(rendered.contains("ab"));
    }

    #[test]
    fn empty_ranges_leave_the_text_untouched() {
        assert_eq!(highlight("abc", &[], true), "abc");
    }
}
