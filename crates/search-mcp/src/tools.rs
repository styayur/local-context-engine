//! The tool surface exposed to MCP clients.
//!
//! Every tool is a thin adapter over [`SearchService`]. There is no search
//! logic here — if a tool needed to make a decision about ranking or matching,
//! that decision would belong in `search-core`, not in the protocol layer.

use search_core::{EntityType, Filter, LceError, MAX_RESULT_LIMIT};
use search_daemon::{SearchOptions, SearchService};
use serde_json::{json, Value};

/// The MCP tools this server advertises.
#[must_use]
pub fn list() -> Vec<Value> {
    vec![
        json!({
            "name": "search_system",
            "description": "Search this machine across files, applications, processes, services and windows. \
        Accepts plain keywords, natural language (English or Simplified Chinese) or the Local Search DSL.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Plain text, natural language or Local Search DSL." },
                    "types": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["file", "directory", "process", "application", "service", "window"] },
                        "description": "Restrict the search to these entity types."
                    },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_RESULT_LIMIT }
                },
                "required": ["query"],
                "additionalProperties": false
            }
        }),
        scoped(
            "search_files",
            "Search indexed files and folders.",
            &["file", "directory"],
        ),
        scoped(
            "search_processes",
            "Search currently running processes.",
            &["process"],
        ),
        scoped(
            "search_apps",
            "Search installed applications.",
            &["application"],
        ),
        scoped("search_services", "Search Windows services.", &["service"]),
        scoped(
            "search_windows",
            "Search open top-level windows.",
            &["window"],
        ),
        json!({
            "name": "compile_query",
            "description": "Compile plain text or natural language into the canonical Local Search DSL without running a search. \
        Use this to see exactly how an input will be interpreted.",
            "inputSchema": {
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "index_status",
            "description": "Report index health: active backend, entry counts, memory footprint, volumes and per-provider state.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        json!({
            "name": "dsl_reference",
            "description": "Return the Local Search DSL reference: supported keys, operators and examples.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        json!({
            "name": "record_selection",
            "description": "Record that a result was chosen, so it ranks higher next time. Purely local.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "entity_id": { "type": "string", "description": "The `id` field of a previous search result." }
                },
                "required": ["query", "entity_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "terminate_process",
            "description": "End a process. Destructive: `confirm` must be true, which the caller should only set after a human approves.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pid": { "type": "integer", "minimum": 1 },
                    "confirm": { "type": "boolean", "description": "Must be true. Set only after explicit human approval." }
                },
                "required": ["pid", "confirm"],
                "additionalProperties": false
            }
        }),
    ]
}

fn scoped(name: &str, description: &str, types: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_RESULT_LIMIT },
                "filters": {
                    "type": "array",
                    "description": "Extra structured predicates, for example [{\"filter\":\"extension\",\"value\":\"rs\"}].",
                    "items": { "type": "object" }
                }
            },
            "required": ["query"],
            "additionalProperties": false,
            "x-entity-types": types
        }
    })
}

/// Dispatch a tool call.
pub fn call(service: &SearchService, name: &str, arguments: &Value) -> Result<Value, LceError> {
    match name {
        "search_system" => search(service, arguments, None),
        "search_files" => search(
            service,
            arguments,
            Some(&[EntityType::File, EntityType::Directory]),
        ),
        "search_processes" => search(service, arguments, Some(&[EntityType::Process])),
        "search_apps" => search(service, arguments, Some(&[EntityType::Application])),
        "search_services" => search(service, arguments, Some(&[EntityType::Service])),
        "search_windows" => search(service, arguments, Some(&[EntityType::Window])),
        "compile_query" => compile(service, arguments),
        "index_status" => Ok(serde_json::to_value(service.index_status()).unwrap_or(Value::Null)),
        "dsl_reference" => Ok(dsl_reference()),
        "record_selection" => record_selection(service, arguments),
        "terminate_process" => terminate(service, arguments),
        other => Err(LceError::Search(search_core::SearchError::InvalidQuery {
            reason: format!("unknown tool `{other}`"),
        })),
    }
}

fn search(
    service: &SearchService,
    arguments: &Value,
    forced_types: Option<&[EntityType]>,
) -> Result<Value, LceError> {
    let query = required_string(arguments, "query")?;
    let limit = arguments.get("limit").and_then(Value::as_u64);

    let mut types: Vec<EntityType> = match forced_types {
        Some(types) => types.to_vec(),
        None => parse_types(arguments.get("types"))?,
    };
    if forced_types.is_none() && types.is_empty() {
        // The caller narrowed nothing, so keep every type in play.
        types = EntityType::ALL.to_vec();
    }

    let extra_filters = parse_filters(arguments.get("filters"))?;
    let options = SearchOptions {
        types,
        limit: limit.map(|value| value as usize),
        explain: false,
    };

    let mut outcome = service.search_detailed(&query, &options);
    if !extra_filters.is_empty() {
        let mut compiled_query = outcome.compiled.query.clone();
        compiled_query.filters.extend(extra_filters);
        let response = service.search_query(&compiled_query);
        outcome.response = response;
    }

    Ok(project(&outcome))
}

fn compile(service: &SearchService, arguments: &Value) -> Result<Value, LceError> {
    let query = required_string(arguments, "query")?;
    let compiled = service.compile(&query);
    Ok(json!({
        "input": compiled.raw,
        "source": compiled.source.as_str(),
        "dsl": compiled.to_dsl(),
        "notes": compiled.notes,
        "entity_types": compiled.query.entity_types,
        "filters": compiled.query.filters,
        "sort": compiled.query.sort,
    }))
}

fn record_selection(service: &SearchService, arguments: &Value) -> Result<Value, LceError> {
    let query = required_string(arguments, "query")?;
    let entity_id = required_string(arguments, "entity_id")?;
    service.record_selection(&query, &entity_id)?;
    Ok(json!({ "recorded": true, "entity_id": entity_id }))
}

fn terminate(service: &SearchService, arguments: &Value) -> Result<Value, LceError> {
    let pid = arguments
        .get("pid")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("`pid` is required and must be an integer"))?;
    let confirm = arguments
        .get("confirm")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let pid = u32::try_from(pid).map_err(|_| invalid("`pid` is out of range"))?;
    service.terminate_process(pid, confirm)?;
    Ok(json!({ "terminated": true, "pid": pid }))
}

/// Project the full response into the compact shape MCP clients consume.
#[must_use]
pub fn project(outcome: &search_daemon::SearchOutcome) -> Value {
    let results: Vec<Value> = outcome
        .response
        .results
        .iter()
        .map(|result| {
            json!({
                "id": result.id,
                "type": result.entity_type.as_str(),
                "name": result.display_name,
                "path": result.path,
                "subtitle": result.subtitle,
                "score": result.score,
                "metadata": result.metadata,
                "match_ranges": result.match_ranges,
            })
        })
        .collect();

    json!({
        "query": outcome.response.query,
        "compiled": outcome.compiled.to_dsl(),
        "source": outcome.compiled.source.as_str(),
        "elapsed_ms": outcome.response.elapsed_ms,
        "total": outcome.response.total,
        "truncated": outcome.response.truncated,
        "results": results,
        "warnings": outcome.response.warnings,
    })
}

fn dsl_reference() -> Value {
    json!({
        "grammar": "token := key \":\" value | word",
        "keys": {
            "type": "file | directory | process | application | service | window (comma separated, repeatable)",
            "ext": "extension without the dot, for example ext:rs",
            "path": "substring of the full path, quote it when it contains spaces",
            "name": "substring of the entity name",
            "drive": "volume letter, for example drive:C",
            "modified": "<24h, >7d, >=2024-01-01, <=2024-01-01",
            "created": "same operators as modified",
            "size": ">10mb, <=512kb, =1gb",
            "state": "running, stopped, paused, auto, manual, disabled",
            "pid": "process id",
            "user": "owning account substring",
            "visible": "true or false, for windows",
            "sort": "relevance | name | path | modified | created | size | pid | memory, with -asc or -desc",
            "limit": "1..2000"
        },
        "examples": [
            "chrome",
            "type:file rust",
            "type:file ext:pdf modified:<24h",
            "type:process python",
            "type:process name:node",
            "type:app vscode",
            "type:service state:running",
            "type:window github",
            "path:projects ext:toml",
            "drive:C ext:exe sort:size-desc",
            "最近的 pdf",
            "正在运行的 python",
            "昨天修改的 rust 文件"
        ],
        "notes": [
            "An unrecognised key is treated as plain text; parsing never fails.",
            "Natural language is compiled by a deterministic rule set, not by a model."
        ]
    })
}

fn required_string(arguments: &Value, key: &str) -> Result<String, LceError> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| invalid(&format!("`{key}` is required and must be a string")))
}

fn parse_types(value: Option<&Value>) -> Result<Vec<EntityType>, LceError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        return Err(invalid("`types` must be an array of strings"));
    };
    let mut types = Vec::new();
    for item in items {
        let Some(name) = item.as_str() else {
            return Err(invalid("`types` must contain only strings"));
        };
        let entity_type = EntityType::parse(name)
            .ok_or_else(|| invalid(&format!("unknown entity type `{name}`")))?;
        if !types.contains(&entity_type) {
            types.push(entity_type);
        }
    }
    Ok(types)
}

fn parse_filters(value: Option<&Value>) -> Result<Vec<Filter>, LceError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        return Err(invalid("`filters` must be an array"));
    };
    let mut filters = Vec::new();
    for item in items {
        let name = item
            .get("filter")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("each filter needs a `filter` name"))?;
        match name {
            "extension" => filters.push(Filter::Extension(
                item.get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("`extension` needs a string `value`"))?
                    .to_string(),
            )),
            "path" => filters.push(Filter::Path(
                item.get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("`path` needs a string `value`"))?
                    .to_string(),
            )),
            "name" => filters.push(Filter::Name(
                item.get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("`name` needs a string `value`"))?
                    .to_string(),
            )),
            "drive" => {
                let letter = item
                    .get("value")
                    .and_then(Value::as_str)
                    .and_then(|value| value.chars().next())
                    .ok_or_else(|| invalid("`drive` needs a single letter"))?;
                filters.push(Filter::Drive(letter));
            }
            "state" => filters.push(Filter::State(
                item.get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("`state` needs a string `value`"))?
                    .to_string(),
            )),
            "pid" => {
                let pid = item
                    .get("value")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(|| invalid("`pid` needs an integer `value`"))?;
                filters.push(Filter::Pid(pid));
            }
            other => {
                return Err(invalid(&format!(
                    "unknown filter `{other}`; try extension, path, name, drive, state or pid"
                )))
            }
        }
    }
    Ok(filters)
}

fn invalid(message: &str) -> LceError {
    LceError::Search(search_core::SearchError::InvalidQuery {
        reason: message.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use search_daemon::{Settings, UsageIndex};
    use serde_json::json;

    fn service() -> SearchService {
        SearchService::new(Settings::default(), UsageIndex::default())
    }

    #[test]
    fn every_required_tool_is_advertised() {
        let names: Vec<String> = list()
            .into_iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
            .collect();
        for required in [
            "search_system",
            "search_files",
            "search_processes",
            "search_apps",
            "search_services",
            "search_windows",
            "compile_query",
            "index_status",
        ] {
            assert!(names.contains(&required.to_string()), "missing {required}");
        }
    }

    #[test]
    fn every_tool_has_a_valid_input_schema() {
        for tool in list() {
            let schema = &tool["inputSchema"];
            assert_eq!(schema["type"], "object", "{}", tool["name"]);
            assert!(schema["properties"].is_object(), "{}", tool["name"]);
        }
    }

    #[test]
    fn search_system_returns_the_documented_envelope() {
        let payload = call(
            &service(),
            "search_system",
            &json!({"query": "node", "limit": 3}),
        )
        .expect("search_system must succeed");
        assert_eq!(payload["query"], "node");
        assert!(payload["results"].is_array());
        assert!(payload["compiled"].is_string());
        assert!(payload["elapsed_ms"].is_number());
    }

    #[test]
    fn scoped_search_tools_restrict_the_entity_types() {
        let payload = call(
            &service(),
            "search_processes",
            &json!({"query": "node", "limit": 5}),
        )
        .unwrap();
        for result in payload["results"].as_array().unwrap() {
            assert_eq!(result["type"], "process");
        }
    }

    #[test]
    fn compile_query_does_not_run_a_search() {
        let payload = call(&service(), "compile_query", &json!({"query": "最近的 pdf"})).unwrap();
        assert_eq!(payload["dsl"], "type:file ext:pdf sort:modified-desc");
        assert_eq!(payload["source"], "natural-language");
        assert!(payload.get("results").is_none());
    }

    #[test]
    fn a_missing_query_is_an_error() {
        assert!(call(&service(), "search_system", &json!({})).is_err());
    }

    #[test]
    fn unknown_types_are_rejected() {
        let error = call(
            &service(),
            "search_system",
            &json!({"query": "x", "types": ["nonsense"]}),
        )
        .unwrap_err();
        assert_eq!(error.code(), "invalid-query");
    }

    #[test]
    fn index_status_reports_the_backend() {
        let payload = call(&service(), "index_status", &json!({})).unwrap();
        assert_eq!(payload["requestedBackend"], "auto");
        assert!(payload["providers"].is_array());
    }

    #[test]
    fn the_dsl_reference_documents_every_key() {
        let payload = call(&service(), "dsl_reference", &json!({})).unwrap();
        let keys = payload["keys"].as_object().unwrap();
        for key in [
            "type", "ext", "path", "name", "drive", "modified", "created", "size", "state", "pid",
            "user", "visible", "sort", "limit",
        ] {
            assert!(keys.contains_key(key), "missing key {key}");
        }
        assert!(!payload["examples"].as_array().unwrap().is_empty());
    }

    #[test]
    fn terminating_without_confirmation_fails() {
        let error = call(
            &service(),
            "terminate_process",
            &json!({"pid": 4, "confirm": false}),
        )
        .unwrap_err();
        assert_eq!(error.code(), "access-denied");
    }

    #[test]
    fn extra_filters_are_applied() {
        let payload = call(
            &service(),
            "search_files",
            &json!({
                "query": "",
                "limit": 5,
                "filters": [{ "filter": "extension", "value": "definitelynotanextension" }]
            }),
        )
        .unwrap();
        assert_eq!(payload["results"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn unknown_tools_are_reported() {
        assert!(call(&service(), "nope", &json!({})).is_err());
    }
}
