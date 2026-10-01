//! `localsearch-mcp` — the Local Context Engine MCP server.
//!
//! Runs on stdio: newline-delimited JSON-RPC in, newline-delimited JSON-RPC
//! out. Logs go to stderr so the protocol stream stays clean.
//!
//! Run it directly to try it:
//!
//! ```text
//! localsearch-mcp
//! {"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
//! {"jsonrpc":"2.0","id":2,"method":"tools/list"}
//! {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_system","arguments":{"query":"vscode"}}}
//! ```

use std::io::{BufRead, Write};

use search_daemon::SearchService;
use serde_json::{json, Value};

mod protocol;
mod tools;

use protocol::{Request, RpcError};

fn main() -> std::process::ExitCode {
    init_tracing();

    let service = SearchService::bootstrap();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                tracing::error!(%error, "failed to read from stdin");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                let error = RpcError {
                    code: -32700,
                    message: format!("parse error: {error}"),
                    data: None,
                };
                write_message(&mut stdout, &protocol::failure(Value::Null, &error));
                continue;
            }
        };

        let is_notification = request.id.is_none();
        let id = request.id.clone().unwrap_or(Value::Null);

        if request.jsonrpc != "2.0" {
            let error = RpcError {
                code: -32600,
                message: format!("unsupported jsonrpc version `{}`", request.jsonrpc),
                data: None,
            };
            if !is_notification {
                write_message(&mut stdout, &protocol::failure(id, &error));
            }
            continue;
        }

        match handle(&service, &request) {
            Some(result) => write_message(&mut stdout, &protocol::success(id, result)),
            None => {
                if !is_notification {
                    let error = RpcError::method_not_found(&request.method);
                    write_message(&mut stdout, &protocol::failure(id, &error));
                }
            }
        }
    }

    // Persist usage history on the way out so selections survive a restart.
    let _ = service.save_usage();
    std::process::ExitCode::SUCCESS
}

/// Handle one request. Returns `None` for notifications and unknown methods.
fn handle(service: &SearchService, request: &Request) -> Option<Value> {
    match request.method.as_str() {
        "initialize" => Some(protocol::initialize_result()),
        "notifications/initialized" | "initialized" => None,
        "ping" => Some(json!({})),
        "tools/list" => Some(json!({ "tools": tools::list() })),
        "tools/call" => Some(handle_tool_call(service, request.params.as_ref())),
        "resources/list" => Some(json!({ "resources": [] })),
        "prompts/list" => Some(json!({ "prompts": [] })),
        _ => None,
    }
}

fn handle_tool_call(service: &SearchService, params: Option<&Value>) -> Value {
    let Some(params) = params else {
        return protocol::tool_error("invalid-params", "`params` is required");
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return protocol::tool_error("invalid-params", "`params.name` is required");
    };
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    match tools::call(service, name, &arguments) {
        Ok(payload) => protocol::tool_result(&payload),
        Err(error) => {
            tracing::warn!(tool = name, code = error.code(), %error, "tool call failed");
            protocol::tool_error(error.code(), error.hint())
        }
    }
}

fn write_message(stdout: &mut std::io::Stdout, message: &Value) {
    let encoded = serde_json::to_string(message).unwrap_or_else(|_| "{}".into());
    let mut lock = stdout.lock();
    let _ = writeln!(lock, "{encoded}");
    let _ = lock.flush();
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("LCE_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        // stdout is the protocol stream; diagnostics must not touch it.
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use search_daemon::{Settings, UsageIndex};

    fn service() -> SearchService {
        SearchService::new(Settings::default(), UsageIndex::default())
    }

    fn request(method: &str, params: Option<Value>) -> Request {
        Request {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: method.into(),
            params,
        }
    }

    #[test]
    fn initialize_is_answered() {
        let result = handle(&service(), &request("initialize", None)).unwrap();
        assert_eq!(result["serverInfo"]["name"], "localsearch");
    }

    #[test]
    fn the_initialized_notification_produces_no_response() {
        assert!(handle(&service(), &request("notifications/initialized", None)).is_none());
    }

    #[test]
    fn ping_is_answered() {
        assert_eq!(
            handle(&service(), &request("ping", None)).unwrap(),
            json!({})
        );
    }

    #[test]
    fn tools_list_returns_tools() {
        let result = handle(&service(), &request("tools/list", None)).unwrap();
        assert!(!result["tools"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_tool_call_is_routed_to_the_service() {
        let params = json!({ "name": "compile_query", "arguments": { "query": "最近的 pdf" } });
        let result = handle(&service(), &request("tools/call", Some(params))).unwrap();
        assert_eq!(result["isError"], false);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("type:file ext:pdf sort:modified-desc"));
    }

    #[test]
    fn a_failing_tool_call_is_reported_as_content_not_a_transport_error() {
        let params = json!({ "name": "search_system", "arguments": {} });
        let result = handle(&service(), &request("tools/call", Some(params))).unwrap();
        assert_eq!(result["isError"], true);
    }

    #[test]
    fn unknown_methods_return_none_so_the_caller_can_reply_with_an_error() {
        assert!(handle(&service(), &request("does/not/exist", None)).is_none());
    }

    #[test]
    fn missing_tool_arguments_are_reported() {
        let result = handle(&service(), &request("tools/call", Some(json!({})))).unwrap();
        assert_eq!(result["isError"], true);
    }

    #[test]
    fn resources_and_prompts_are_empty_but_valid() {
        assert_eq!(
            handle(&service(), &request("resources/list", None)).unwrap(),
            json!({ "resources": [] })
        );
        assert_eq!(
            handle(&service(), &request("prompts/list", None)).unwrap(),
            json!({ "prompts": [] })
        );
    }
}
