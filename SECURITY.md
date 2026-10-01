# Security

## Threat model

Local Context Engine reads from the operating system and, on request, opens or
reveals the things it finds. It is not a sandbox and it does not try to be one.

What it does:

* Reads the NTFS master file table or walks directories to build a **name** index.
* Reads live process, window, service and Start Menu snapshots.
* Opens files, folders and applications **only** when you activate a result.
* Ends a process **only** after you accept a confirmation dialog, and the Rust
  side refuses the call unless it was explicitly confirmed.

What it never does:

* It never executes shell commands derived from a query.
* It never evaluates query text.
* It never deserialises an executable payload. Persisted state is a
  MessagePack index file and two JSON files this project wrote.
* It never opens a socket. There is no telemetry, no analytics and no update
  check.
* It never uploads a file list, a query, a query history or a process list.

## Path handling

Paths are treated as opaque UTF-16/UTF-8 strings throughout:

* Spaces, Simplified Chinese, emoji and other Unicode are covered by tests.
* Length is not assumed. The index stores file names in a string arena and
  reconstructs paths on demand, so a very long path is a `String`, not a
  fixed-size buffer.
* Junction points and symbolic links are never followed, so a directory scan
  cannot loop forever.
* Destructive operations are restricted to a single API
  (`actions::terminate_process`) whose signature requires `confirmed = true`.

## Untrusted input

The MCP server accepts JSON-RPC from a local client and treats every field as
untrusted:

* Tool arguments are parsed with explicit type checks; a wrong type is an error,
  never a coercion.
* Unknown tools, unknown filter names and unknown entity types are rejected.
* Query text is never interpolated into SQL, a shell or a path; it only ever
  reaches the matcher.

## Permissions

The index backends are honest about what they need:

| backend   | requirement                                        |
|-----------|----------------------------------------------------|
| `scan`    | none beyond the user's own read access             |
| `mft-usn` | administrator rights to open `\\.\C:`              |

The default is `auto`, which uses `mft-usn` when it is available and otherwise
falls back to `scan`. A denied volume produces a translated, actionable message
in the UI and a full error in the developer log — never a bare `OsError(5)`.

## Reporting a vulnerability

Open a private security advisory on GitHub rather than a public issue. Please
include the version, the Windows build, and the smallest reproduction you can
manage.