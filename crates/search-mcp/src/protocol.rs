//! The slice of the Model Context Protocol this server implements.
//!
//! MCP over stdio is newline-delimited JSON-RPC 2.0. Only four methods are
//! needed to expose tools: `initialize`, `notifications/initialized`,
//! `tools/list` and `tools/call` — plus `ping`, which clients use as a
//! liveness probe.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The protocol revision this server speaks.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// A JSON-RPC request.
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    /// Always `"2.0"`.
    #[serde(default)]
    pub jsonrpc: String,
    /// Present for requests, absent for notifications.
    #[serde(default)]
    pub id: Option<Value>,
    /// The method name.
    pub method: String,
    /// Method parameters.
    #[serde(default)]
    pub params: Option<Value>,
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, Serialize)]
pub struct RpcError {
    /// Numeric code.
    pub code: i64,
    /// Human readable message.
    pub message: String,
    /// Optional structured detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// The method does not exist.
    #[must_use]
    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("unknown method `{method}`"),
            data: None,
        }
    }
}

/// A successful JSON-RPC response.
#[must_use]
pub fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// A failed JSON-RPC response.
#[must_use]
pub fn failure(id: Value, error: &RpcError) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

/// The `initialize` result.
#[must_use]
pub fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "localsearch",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": "Local Context Engine searches this machine offline. \
    Call `search_system` to search files, applications, processes, services and windows at once, \
    or `compile_query` first to see how natural language becomes a deterministic query. \
    Terminating a process needs `confirm: true`."
    })
}

/// The `tools/call` result for a JSON payload.
#[must_use]
pub fn tool_result(payload: &Value) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(payload).unwrap_or_else(|_| "{}".into()),
        }],
        "isError": false,
    })
}

/// The `tools/call` result for a failed tool call.
#[must_use]
pub fn tool_error(code: &str, message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": format!("{code}: {message}") }],
        "isError": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_parse_with_and_without_ids() {
        let request: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).unwrap();
        assert_eq!(request.method, "ping");
        assert!(request.id.is_some());

        let notification: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .unwrap();
        assert!(notification.id.is_none());
    }

    #[test]
    fn success_envelopes_carry_the_id() {
        let value = success(json!(7), json!({"ok": true}));
        assert_eq!(value["id"], 7);
        assert_eq!(value["result"]["ok"], true);
    }

    #[test]
    fn failure_envelopes_carry_a_code() {
        let error = RpcError::method_not_found("nope");
        let value = failure(json!("abc"), &error);
        assert_eq!(value["error"]["code"], -32601);
        assert_eq!(value["id"], "abc");
    }

    #[test]
    fn initialize_advertises_the_tools_capability() {
        let result = initialize_result();
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], "localsearch");
    }

    #[test]
    fn tool_results_are_text_content_blocks() {
        let value = tool_result(&json!({"hello": "world"}));
        assert_eq!(value["isError"], false);
        assert_eq!(value["content"][0]["type"], "text");
        assert!(value["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("hello"));
    }

    #[test]
    fn tool_errors_are_flagged() {
        let value = tool_error("invalid-query", "empty query");
        assert_eq!(value["isError"], true);
        assert!(value["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("invalid-query"));
    }
}
