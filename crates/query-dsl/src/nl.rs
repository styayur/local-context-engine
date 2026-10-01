//! The rule-based natural language query compiler.
//!
//! This is deliberately not a language model. It is a small, inspectable set of
//! phrase detectors that maps the way people actually phrase a desktop search
//! into a [`SearchQuery`]. It handles Simplified Chinese and English, and both
//! languages compile to *exactly the same* query for the same intent, which is
//! what the tests assert.
//!
//! A future AI-backed [`crate::QueryCompiler`] can replace this module. The
//! search engine never learns about it.

use std::time::Duration;

use search_core::{EntityType, Filter, SearchQuery, Sort, TimeBound, DEFAULT_RESULT_LIMIT};

/// What the natural language compiler produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NlOutcome {
    /// The compiled query.
    pub query: SearchQuery,
    /// Whether any intent was recognised at all.
    pub matched: bool,
    /// Human-readable notes, surfaced by `--explain`.
    pub notes: Vec<String>,
}

const SERVICE_PHRASES: &[&str] = &["服务", "services", "service", "svc"];

const PROCESS_PHRASES: &[&str] = &[
    "正在运行",
    "正在执行",
    "运行中的",
    "运行中",
    "进程中",
    "进程的",
    "进程",
    "currently running",
    "processes",
    "process",
    "running",
    "live",
];

const WINDOW_PHRASES: &[&str] = &["窗口", "视窗", "title bar", "window"];

const APP_PHRASES: &[&str] = &[
    "应用程序",
    "应用",
    "软件",
    "程序",
    "applications",
    "application",
    "apps",
    "app",
    "programs",
    "program",
];

const DIRECTORY_PHRASES: &[&str] = &[
    "文件夹",
    "资料夹",
    "目录",
    "folders",
    "folder",
    "directories",
    "directory",
    "dirs",
];

const FILE_PHRASES: &[&str] = &["文件", "档案", "files", "file"];

/// Words that mean "ordered by recency".
const RECENCY_PHRASES: &[&str] = &[
    "最近修改的",
    "最近更新的",
    "最近的",
    "最新的",
    "刚刚",
    "最近",
    "最新",
    "recently modified",
    "recently changed",
    "most recent",
    "recently",
    "recent",
    "latest",
    "newest",
];

/// Verbs that mean "the timestamp that matters is the modified time".
const MODIFY_PHRASES: &[&str] = &[
    "修改过的",
    "修改的",
    "修改",
    "更改过的",
    "更改的",
    "更改",
    "编辑过",
    "编辑",
    "更新过",
    "更新的",
    "更新",
    "modified",
    "changed",
    "edited",
    "updated",
    "touched",
];

/// Relative time windows, mapped to the bounds the engine understands.
const TODAY_PHRASES: &[&str] = &["今天", "今日", "today"];
const YESTERDAY_PHRASES: &[&str] = &["昨天", "昨日", "yesterday"];
const THIS_WEEK_PHRASES: &[&str] = &[
    "本周",
    "这周",
    "这个星期",
    "过去一周",
    "最近一周",
    "最近七天",
    "this week",
    "past week",
    "last week",
    "past 7 days",
    "last 7 days",
];

/// Fillers that carry no search value once the intent is known.
const FILLER_PHRASES: &[&str] = &[
    "帮我找一下",
    "帮我找",
    "帮我",
    "我想找",
    "我想要",
    "我要",
    "请找",
    "查找",
    "搜索",
    "显示",
    "列出",
    "找到",
    "找一下",
    "找",
    "一下",
    "所有",
    "全部",
    "这个",
    "那个",
    "please find",
    "please",
    "look for",
    "search for",
    "where is",
    "show me",
    "show",
    "list",
    "find",
    "search",
    "open",
    "launch",
    "all",
    "the",
    "for",
    "me",
];

/// Unambiguous file extensions. Seeing one of these makes the query about
/// files even without the word "file".
const EXTENSIONS: &[&str] = &[
    "7z", "bat", "bmp", "cpp", "cs", "css", "csv", "dart", "dll", "doc", "docx", "exe", "gif",
    "go", "hpp", "html", "ico", "ini", "java", "jpeg", "jpg", "js", "json", "jsx", "kt", "log",
    "lua", "md", "mkv", "mov", "mp3", "mp4", "pdf", "php", "png", "ppt", "pptx", "ps1", "py",
    "rar", "rb", "rs", "sh", "sql", "svg", "swift", "tar", "toml", "ts", "tsx", "txt", "vue",
    "wav", "webp", "xls", "xlsx", "xml", "yaml", "yml", "zip",
];

/// Language and format names that imply an extension, but only once the query
/// already says it is about files. Without that context `python` should search
/// for the process and the file name, not silently become `ext:py`.
const LANGUAGE_EXTENSIONS: &[(&str, &str)] = &[
    ("visual basic", "vb"),
    ("typescript", "ts"),
    ("javascript", "js"),
    ("powershell", "ps1"),
    ("markdown", "md"),
    ("golang", "go"),
    ("python", "py"),
    ("rust", "rs"),
    ("java", "java"),
    ("kotlin", "kt"),
    ("ruby", "rb"),
    ("yaml", "yml"),
    ("csharp", "cs"),
    ("c#", "cs"),
    ("c++", "cpp"),
    ("shell", "sh"),
    ("bash", "sh"),
    ("json", "json"),
    ("toml", "toml"),
    ("html", "html"),
    ("css", "css"),
    ("sql", "sql"),
    ("php", "php"),
    ("perl", "pl"),
    ("scala", "scala"),
    ("elixir", "ex"),
    ("haskell", "hs"),
    ("lua", "lua"),
    ("dart", "dart"),
    ("matlab", "m"),
];

/// Compile free-form text into a [`SearchQuery`].
#[must_use]
pub fn compile_natural_language(input: &str) -> NlOutcome {
    let raw = input.trim();
    let mut notes: Vec<String> = Vec::new();
    if raw.is_empty() {
        return NlOutcome {
            query: SearchQuery {
                limit: DEFAULT_RESULT_LIMIT,
                ..SearchQuery::default()
            },
            matched: false,
            notes,
        };
    }

    let lower = raw.to_lowercase();

    // --- 1. Which domain is the user asking about? -----------------------
    // Most specific domain words are checked first so that `正在运行的服务`
    // ("running services") resolves to services rather than processes.
    let mut entity_types: Vec<EntityType> = Vec::new();
    let service_intent = contains_phrase(&lower, SERVICE_PHRASES) && !lower.contains("服务器");
    if service_intent {
        entity_types.push(EntityType::Service);
        notes.push("intent: service".into());
    } else if contains_phrase(&lower, PROCESS_PHRASES) {
        entity_types.push(EntityType::Process);
        notes.push("intent: process".into());
    } else if contains_phrase(&lower, WINDOW_PHRASES) {
        entity_types.push(EntityType::Window);
        notes.push("intent: window".into());
    } else if contains_phrase(&lower, APP_PHRASES) {
        entity_types.push(EntityType::Application);
        notes.push("intent: application".into());
    } else if contains_phrase(&lower, DIRECTORY_PHRASES) {
        entity_types.push(EntityType::Directory);
        notes.push("intent: folder".into());
    }

    // --- 2. Recency and modification verbs -------------------------------
    let recency = contains_phrase(&lower, RECENCY_PHRASES);
    let modified_verb = contains_phrase(&lower, MODIFY_PHRASES);
    let file_context = contains_phrase(&lower, FILE_PHRASES)
        || contains_phrase(&lower, DIRECTORY_PHRASES)
        || matches!(
            entity_types.first(),
            Some(EntityType::File | EntityType::Directory)
        );

    // --- 3. Extension ----------------------------------------------------
    // Emitted before the time bound so the canonical DSL reads
    // `ext:rs modified:<24h`, matching the documented examples.
    let mut filters: Vec<Filter> = Vec::new();
    let extension = detect_extension(&lower, file_context);
    let mut language_hit: Option<&'static str> = None;
    if let Some(ext) = extension.as_deref() {
        filters.push(Filter::Extension(ext.to_string()));
        notes.push(format!("extension: {ext}"));
        language_hit = LANGUAGE_EXTENSIONS
            .iter()
            .find(|(_, mapped)| *mapped == ext)
            .map(|(name, _)| *name);
    }

    // --- 4. Explicit time window -----------------------------------------
    let mut sort: Option<Sort> = None;
    if let Some(duration) = relative_window(&lower) {
        filters.push(Filter::Modified(TimeBound::Within(duration)));
        notes.push(format!(
            "time: modified within {}",
            search_core::clock::format_duration(duration)
        ));
    } else if recency && modified_verb {
        filters.push(Filter::Modified(TimeBound::Within(Duration::from_secs(
            7 * 86_400,
        ))));
        notes.push("time: modified within 7d".into());
    }

    // A file-shaped query defaults to files; an explicit other domain wins.
    if entity_types.is_empty() && (extension.is_some() || recency || file_context) {
        entity_types.push(EntityType::File);
        notes.push("intent: file".into());
    }

    // --- 5. Sort ----------------------------------------------------------
    if recency {
        sort = Some(Sort::modified_desc());
        notes.push("sort: modified-desc".into());
    }

    // --- 6. Strip recognised phrases to leave the subject -----------------
    let mut residual = strip_past_days(&lower);
    for group in [
        SERVICE_PHRASES,
        PROCESS_PHRASES,
        WINDOW_PHRASES,
        APP_PHRASES,
        DIRECTORY_PHRASES,
        FILE_PHRASES,
        RECENCY_PHRASES,
        MODIFY_PHRASES,
        TODAY_PHRASES,
        YESTERDAY_PHRASES,
        THIS_WEEK_PHRASES,
        FILLER_PHRASES,
    ] {
        for phrase in group {
            residual = strip_phrase(&residual, phrase);
        }
    }
    residual = strip_phrase(&residual, "的");
    residual = strip_phrase(&residual, "了");

    // Drop the token that produced the extension so it does not also act as
    // free text ("recent PDFs" must not also require the literal word "pdfs").
    if let Some(ext) = extension.as_deref() {
        residual = strip_token(&residual, ext);
        residual = strip_token(&residual, &format!("{ext}s"));
    }
    if let Some(name) = language_hit {
        residual = strip_phrase(&residual, name);
        for word in name.split_whitespace() {
            residual = strip_token(&residual, word);
        }
    }

    let text = collapse_whitespace(&residual);
    // Recognising "find"/"找"/"please" is still recognition: it changes the
    // subject, so the compiler claims the input rather than passing it through
    // untranslated.
    let matched = !entity_types.is_empty()
        || !filters.is_empty()
        || sort.is_some()
        || text != collapse_whitespace(&lower);

    let query = SearchQuery {
        text: (!text.is_empty()).then_some(text),
        entity_types,
        filters,
        sort,
        limit: DEFAULT_RESULT_LIMIT,
    };

    NlOutcome {
        query,
        matched,
        notes,
    }
}

fn relative_window(lower: &str) -> Option<Duration> {
    if contains_phrase(lower, TODAY_PHRASES) || contains_phrase(lower, YESTERDAY_PHRASES) {
        return Some(Duration::from_secs(24 * 3_600));
    }
    if contains_phrase(lower, THIS_WEEK_PHRASES) {
        return Some(Duration::from_secs(7 * 86_400));
    }
    parse_past_days(lower).map(|days| Duration::from_secs(days * 86_400))
}

/// Recognise `过去 30 天`, `过去30天`, `past 30 days`, `last 14 days`.
fn parse_past_days(lower: &str) -> Option<u64> {
    for marker in ["过去", "最近", "past", "last"] {
        let Some(start) = lower.find(marker) else {
            continue;
        };
        let rest = &lower[start + marker.len()..];
        let after_marker = rest.trim_start();
        let digits: String = after_marker
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if digits.is_empty() {
            continue;
        }
        let Ok(days) = digits.parse::<u64>() else {
            continue;
        };
        if days == 0 || days > 3_650 {
            continue;
        }
        let after_digits = after_marker[digits.len()..].trim_start();
        if starts_with_unit(after_digits) {
            return Some(days);
        }
    }
    None
}

fn starts_with_unit(value: &str) -> bool {
    value.starts_with('天')
        || value.starts_with('日')
        || value.to_ascii_lowercase().starts_with("days")
        || value.to_ascii_lowercase().starts_with("day")
        || value.to_ascii_lowercase().starts_with('d')
}

/// Remove a `过去 N 天` / `past N days` span from the residual.
fn strip_past_days(haystack: &str) -> String {
    let mut result = haystack.to_string();
    for marker in ["过去", "最近", "past", "last"] {
        let Some(start) = result.find(marker) else {
            continue;
        };
        let rest = &result[start + marker.len()..];
        let ws1 = rest.len() - rest.trim_start().len();
        let after_ws = &rest[ws1..];
        let digits_len = after_ws.chars().take_while(char::is_ascii_digit).count();
        if digits_len == 0 {
            continue;
        }
        let after_digits = &after_ws[digits_len..];
        let ws2 = after_digits.len() - after_digits.trim_start().len();
        let unit_src = &after_digits[ws2..];
        let unit_len = if unit_src.starts_with('天') || unit_src.starts_with('日') {
            3
        } else if unit_src.len() >= 4 && unit_src[..4].eq_ignore_ascii_case("days") {
            4
        } else if unit_src.len() >= 3 && unit_src[..3].eq_ignore_ascii_case("day") {
            3
        } else if !unit_src.is_empty() && unit_src[..1].eq_ignore_ascii_case("d") {
            1
        } else {
            continue;
        };
        let end = start + marker.len() + ws1 + digits_len + ws2 + unit_len;
        let mut next = String::with_capacity(result.len());
        next.push_str(&result[..start]);
        next.push(' ');
        next.push_str(&result[end..]);
        result = next;
    }
    result
}

fn detect_extension(lower: &str, file_context: bool) -> Option<String> {
    // Explicit extensions win, and are unambiguous on their own.
    for token in word_tokens(lower) {
        let singular = singularize(&token);
        if EXTENSIONS.contains(&singular.as_str()) {
            return Some(singular);
        }
    }
    if !file_context {
        return None;
    }
    // Multi-word language names next.
    for (name, extension) in LANGUAGE_EXTENSIONS {
        if name.contains(' ') && contains_literal(lower, name) {
            return Some((*extension).to_string());
        }
    }
    for token in word_tokens(lower) {
        let singular = singularize(&token);
        if let Some((_, extension)) = LANGUAGE_EXTENSIONS
            .iter()
            .find(|(name, _)| *name == singular)
        {
            return Some((*extension).to_string());
        }
    }
    // Symbolic names such as `c++` and `c#` are not word tokens.
    for (name, extension) in LANGUAGE_EXTENSIONS {
        if !name.contains(' ') && !name.chars().all(char::is_alphanumeric) && lower.contains(name) {
            return Some((*extension).to_string());
        }
    }
    None
}

fn contains_literal(haystack: &str, needle: &str) -> bool {
    haystack.contains(needle)
}

fn singularize(token: &str) -> String {
    if let Some(stripped) = token.strip_suffix("ies") {
        if stripped.len() >= 2 {
            return format!("{stripped}y");
        }
    }
    if let Some(stripped) = token.strip_suffix("es") {
        if EXTENSIONS.contains(&stripped) {
            return stripped.to_string();
        }
    }
    if let Some(stripped) = token.strip_suffix('s') {
        if stripped.len() >= 2 {
            return stripped.to_string();
        }
    }
    token.to_string()
}

fn word_tokens(value: &str) -> Vec<String> {
    value
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '+')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn contains_phrase(haystack: &str, phrases: &[&str]) -> bool {
    phrases
        .iter()
        .any(|phrase| contains_bounded(haystack, phrase))
}

/// Substring test that respects ASCII word boundaries but lets CJK phrases sit
/// flush against surrounding characters (`rust文件`).
fn contains_bounded(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let cjk = !needle.is_ascii();
    let mut from = 0usize;
    while from < haystack.len() {
        let Some(offset) = haystack[from..].find(needle) else {
            return false;
        };
        let start = from + offset;
        let end = start + needle.len();
        let before_ok = start == 0
            || !haystack[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        let after_ok = end == haystack.len()
            || !haystack[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        if cjk || (before_ok && after_ok) {
            return true;
        }
        from = end;
    }
    false
}

/// Remove every bounded occurrence of `needle`, replacing it with a space.
fn strip_phrase(haystack: &str, needle: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let cjk = !needle.is_ascii();
    let mut result = String::with_capacity(haystack.len());
    let mut rest = haystack;
    while let Some(start) = rest.find(needle) {
        let end = start + needle.len();
        let before_ok = start == 0
            || !rest[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        let after_ok = end == rest.len()
            || !rest[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        if cjk || (before_ok && after_ok) {
            result.push_str(&rest[..start]);
            result.push(' ');
            rest = &rest[end..];
        } else {
            result.push_str(&rest[..end]);
            rest = &rest[end..];
        }
    }
    result.push_str(rest);
    result
}

/// Remove a whole token from the residual.
fn strip_token(haystack: &str, token: &str) -> String {
    if token.is_empty() {
        return haystack.to_string();
    }
    word_tokens(haystack)
        .into_iter()
        .filter(|candidate| candidate != token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_pdfs_chinese_and_english_compile_identically() {
        let zh = compile_natural_language("最近的 PDF");
        let en = compile_natural_language("recent PDFs");
        assert_eq!(zh.query, en.query);
        assert_eq!(zh.query.entity_types, vec![EntityType::File]);
        assert_eq!(zh.query.filters, vec![Filter::Extension("pdf".into())]);
        assert_eq!(zh.query.sort, Some(Sort::modified_desc()));
        assert_eq!(zh.query.text, None);
    }

    #[test]
    fn running_python_chinese_and_english_compile_identically() {
        let zh = compile_natural_language("正在运行的 python");
        let en = compile_natural_language("running python processes");
        assert_eq!(zh.query, en.query);
        assert_eq!(zh.query.entity_types, vec![EntityType::Process]);
        assert_eq!(zh.query.text.as_deref(), Some("python"));
        assert!(zh.query.filters.is_empty());
    }

    #[test]
    fn find_vscode_chinese_and_english_compile_identically() {
        let zh = compile_natural_language("找 VS Code");
        let en = compile_natural_language("find VS Code");
        assert_eq!(zh.query, en.query);
        assert!(zh.query.entity_types.is_empty());
        assert_eq!(zh.query.text.as_deref(), Some("vs code"));
    }

    #[test]
    fn yesterday_modified_rust_files_adds_a_24_hour_bound() {
        let outcome = compile_natural_language("昨天修改的 rust 文件");
        assert_eq!(outcome.query.entity_types, vec![EntityType::File]);
        assert_eq!(outcome.query.filters[0], Filter::Extension("rs".into()));
        assert_eq!(
            outcome.query.filters[1],
            Filter::Modified(TimeBound::Within(Duration::from_secs(24 * 3_600)))
        );
        assert_eq!(outcome.query.text, None);
    }

    #[test]
    fn plain_keywords_are_not_rewritten() {
        let outcome = compile_natural_language("vscode");
        assert!(!outcome.matched);
        assert!(outcome.query.entity_types.is_empty());
        assert_eq!(outcome.query.text.as_deref(), Some("vscode"));
    }

    #[test]
    fn python_alone_stays_a_keyword() {
        let outcome = compile_natural_language("python");
        assert!(outcome.query.filters.is_empty());
        assert_eq!(outcome.query.text.as_deref(), Some("python"));
    }

    #[test]
    fn service_intent_wins_over_the_running_verb() {
        let outcome = compile_natural_language("正在运行的服务");
        assert_eq!(outcome.query.entity_types, vec![EntityType::Service]);
        assert_eq!(outcome.query.text, None);
    }

    #[test]
    fn window_intent_is_detected() {
        let outcome = compile_natural_language("github 窗口");
        assert_eq!(outcome.query.entity_types, vec![EntityType::Window]);
        assert_eq!(outcome.query.text.as_deref(), Some("github"));
    }

    #[test]
    fn past_days_window_is_parsed() {
        let outcome = compile_natural_language("past 30 days rust files");
        assert!(outcome
            .query
            .filters
            .contains(&Filter::Modified(TimeBound::Within(Duration::from_secs(
                30 * 86_400
            )))));
        assert_eq!(outcome.query.text, None);
    }

    #[test]
    fn chinese_past_days_window_is_parsed() {
        let outcome = compile_natural_language("过去7天修改的pdf文件");
        assert_eq!(
            outcome.query.filters,
            vec![
                Filter::Extension("pdf".into()),
                Filter::Modified(TimeBound::Within(Duration::from_secs(7 * 86_400)))
            ]
        );
        assert_eq!(outcome.query.text, None);
    }

    #[test]
    fn language_names_only_become_extensions_in_file_context() {
        let file_query = compile_natural_language("rust 文件");
        assert_eq!(
            file_query.query.filters,
            vec![Filter::Extension("rs".into())]
        );
        let process_query = compile_natural_language("running rust");
        assert_eq!(process_query.query.entity_types, vec![EntityType::Process]);
        assert!(process_query.query.filters.is_empty());
        assert_eq!(process_query.query.text.as_deref(), Some("rust"));
    }

    #[test]
    fn does_not_break_words_that_contain_keywords() {
        let outcome = compile_natural_language("runtime");
        assert_eq!(outcome.query.text.as_deref(), Some("runtime"));
    }

    #[test]
    fn filler_words_are_removed_from_the_subject() {
        let outcome = compile_natural_language("please find Visual Studio Code");
        assert_eq!(outcome.query.text.as_deref(), Some("visual studio code"));
    }
}
