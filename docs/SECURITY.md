# Security and privacy

## Local data and network boundary

Default search runs locally, with no account, telemetry, source upload or model
call. It indexes names, paths and metadata, not file contents. Settings, index
caches and optional bounded selection history are local data; filenames alone
can disclose sensitive information. Usage history can be cleared with
`localsearch --reset-usage`. MCP communicates over local stdio, but the MCP
client controls where it forwards returned results. Opening an application or
file may start software with its own network behavior. Build/package downloads
are separate from offline runtime search.

## MFT and permissions

Ordinary directory scanning uses the current user's access rights. Raw NTFS MFT
enumeration requires elevated volume access and **can disclose names inside
folders the searching user cannot normally list**. Opening file contents still
goes through Windows access checks. If this metadata exposure is unacceptable,
use unprivileged `scan` and do not install the index service.

The optional service owns raw handles and journal workers; CLI/Desktop/MCP can
remain ordinary-user processes. Service installation/start/stop are explicit
administrative actions. The local named pipe has an explicit DACL for SYSTEM,
Administrators and the installing user's SID, rejects remote clients and uses
first-instance creation. A permitted same-user process can also connect; this
is not caller attestation or an OS sandbox.

The protocol has five requests: Hello, Search, IndexStatus, RebuildVolume and
Changes. It has no shell, execute or file-content request. Frames have magic and
version checks and an 8 MiB allocation ceiling. Search limits are bounded. A
protocol mismatch or service failure is surfaced and the user-side provider can
fall back; confirm the actual backend and health after failure.

## Actions and untrusted input

Query text is data, not a shell command. MCP arguments and pipe frames are
validated. Directory scans do not follow junctions/symlinks. File opening and
folder reveal are explicit actions. Process termination requires confirmation
at the interface and Rust action boundary; it can still lose unsaved work.
Services are inspected, not controlled, by the search provider.

A raw index is not a per-result authorization system. Protect local cache files
and review service deployment on shared machines. The project does not claim a
complete independent security audit. Report vulnerabilities using the root
[security policy](../SECURITY.md).
