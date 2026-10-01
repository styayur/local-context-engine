# MCP server

`localsearch-mcp.exe` speaks the Model Context Protocol over stdio, so an agent
can ask this machine a question without any network access at all.

## Running it

```jsonc
// Claude Desktop / Cursor / any MCP client configuration
{
  "mcpServers": {
    "localsearch": {
      "command": "C:\\path\\to\\localsearch-mcp.exe"
    }
  }
}
```

Try it by hand — one JSON-RPC message per line:

```powershell
localsearch-mcp
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_system","arguments":{"query":"vscode","limit":5}}}
```

Diagnostics go to stderr; stdout carries nothing but protocol messages.

## Tools

| tool                | arguments                                   | returns                                     |
|---------------------|---------------------------------------------|---------------------------------------------|
| `search_system`     | `query`, `types?`, `limit?`                 | results across every domain                 |
| `search_files`      | `query`, `limit?`, `filters?`               | files and folders                           |
| `search_processes`  | `query`, `limit?`, `filters?`               | running processes                           |
| `search_apps`       | `query`, `limit?`, `filters?`               | installed applications                      |
| `search_services`   | `query`, `limit?`, `filters?`               | Windows services                            |
| `search_windows`    | `query`, `limit?`, `filters?`               | open top-level windows                      |
| `compile_query`     | `query`                                     | the compiled DSL, the source and the notes   |
| `index_status`      | –                                           | backend, entry counts, memory, providers     |
| `dsl_reference`     | –                                           | the key table and examples                  |
| `record_selection`  | `query`, `entity_id`                        | boosts that result next time                 |
| `terminate_process` | `pid`, `confirm`                            | ends a process; `confirm` must be `true`     |

`query` accepts plain keywords, natural language or the Local Search DSL.

### Example

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "search_system",
    "arguments": {
      "query": "python",
      "types": ["process", "file"],
      "limit": 20
    }
  }
}
```

Response (abridged):

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "content": [
      {
        "type": "text",
        "text": "{\n  \"compiled\": \"type:process,file python limit:20\",\n  \"results\": [ ... ]\n}"
      }
    ],
    "isError": false
  }
}
```

## Design notes

* The layer is a **pure adapter**. Every tool calls `SearchService`; none of them
  reimplements matching, filtering or ranking.
* A tool failure is reported as `isError: true` with the stable error code in
  the text, not as a transport error — clients show it to the model, which is
  what you want for "the index is not built yet".
* A malformed JSON line is a JSON-RPC parse error; a wrong argument type is a
  tool error. Neither can panic the server.
* `terminate_process` exists because the spec asks for a complete action
  surface, and it is deliberately awkward: the caller has to pass
  `confirm: true`, and the Rust side refuses everything else.