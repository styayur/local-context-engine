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
---

# Threat model: the privileged index service (v0.2)

Version 0.2 moves the operations that need administrator rights out of the
desktop, CLI and MCP processes and into one small service. That is a smaller
attack surface, but it is a trust boundary, and boundaries deserve to be written
down rather than assumed.

## What runs elevated, and what does not

```text
Desktop ─┐
CLI ─────┼── ordinary user, no UAC prompt, no admin token
MCP ─────┘
     │
     │ Named Pipe  \\.\pipe\localsearch-index-v1
     ▼
lce-index-service.exe ── LocalSystem / Administrator
     ├── open \\.\C: and enumerate the MFT
     ├── read and tail the USN change journals
     └── answer index queries
```

The service exists because opening `\\.\C:` for `FILE_READ_DATA` requires
elevation. Nothing else does: opening files, revealing folders and ending
processes all stay in the unprivileged processes, where they belong.

## What the service can be asked to do

Exactly five things, and the list is the whole protocol:

| request          | effect                                                  |
|------------------|---------------------------------------------------------|
| `Hello`          | handshake: protocol version, service version, volumes    |
| `Search`         | run a query against the service's index                  |
| `IndexStatus`    | report backend, entry counts, per-volume sync state      |
| `RebuildVolume`  | re-enumerate the MFT for one volume (or all)             |
| `Changes`        | poll for journal advances since a cursor                 |

There is no `Execute`, no `Shell`, no `Command`, no `WriteFile` and no `Open`.
This is not a policy that a reviewer has to check line by line: a message that
cannot be constructed cannot be handled. `crates/index-protocol` has a test that
enumerates the variants so that adding a dangerous one is a visible diff.

## Raw MFT visibility: the honest statement

**MFT enumeration can reveal file names that the calling user could not discover
by walking the directory tree.**

The master file table does not apply per-directory ACLs. A volume-wide
enumeration returns every name on the volume, including names inside directories
the user cannot open. This is inherent to the technique — the same is true of
`Everything`, of `fsutil usn readjournal`, and of any MFT-based indexer — and it
is *not* something this project can fix by being careful.

The consequences, stated plainly:

* An index built by the service contains names the user may not be permitted to
  read. Search results can therefore surface a path whose contents will still be
  denied on open, because the open goes through the normal ACL check.
* The service does not read file *contents*, and it never returns file
  contents. It returns names, paths and metadata.
* On a machine where more than one person has an account, that is a real
  disclosure of *names*. If that is unacceptable for a given machine, do not
  install the service: the unprivileged directory-scan backend indexes exactly
  what the user can already see.

## Pipe access control

The DACL is built from an SDDL string, not assembled by hand:

```text
D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;<the installing user's SID>)
```

* `SY` — LocalSystem, which owns the service.
* `BA` — the built-in Administrators group, which may manage it.
* the installing user's SID — read and write, enough to ask a question.

Deliberately absent: `WD` (Everyone) and `AN` (Anonymous). A pipe created with a
default security descriptor grants both, and that is the single most common
mistake in this kind of design.

Additional hardening, all in `crates/index-service/src/pipe.rs`:

* `FILE_FLAG_FIRST_PIPE_INSTANCE` — if another process already owns the name,
  creating the pipe **fails** instead of silently sharing. This defeats pipe
  name squatting, where a hostile local process creates the pipe first and
  harvests whatever the front ends send it.
* `PIPE_REJECT_REMOTE_CLIENTS` — remote clients are refused by the pipe itself,
  not merely by the ACL, so the boundary does not depend on the DACL being
  perfect.
* Byte-mode pipes with an explicit length prefix, so a half-written message is a
  detectable truncated frame rather than a silently short read.

## Malformed client input

Every field that crosses the pipe is treated as hostile:

| input                       | handling                                             |
|-----------------------------|------------------------------------------------------|
| bad frame magic             | `BadMagic`, connection dropped                        |
| unknown protocol version    | `VersionMismatch`, connection dropped                 |
| frame larger than 8 MiB     | `TooLarge`, **rejected before the body is allocated** |
| truncated frame             | `Truncated`, connection dropped                       |
| malformed MessagePack body  | `Malformed`, connection dropped                       |
| unknown request variant     | decode failure, connection dropped                     |
| unknown entity type in `Search` | ignored; the type filter simply does not match it  |
| impossible `limit`          | clamped to the service's own maximum (2 000 hits)     |

The service never unwraps a client-controlled value and never indexes a buffer
with a client-supplied length: the frame reader checks the announced length
against its own constant before allocating.

## Protocol downgrade

A downgrade is refused in two places:

1. **Transport.** Every frame carries its version, and a mismatch is an error
   before the body is even parsed.
2. **Application.** `Hello` carries the client's version and the service answers
   `protocol-mismatch` if they disagree, so a client that somehow negotiated the
   wrong version at the transport layer still cannot proceed.

There is no "compatibility mode" and no best-effort decode. If the revisions
differ, the front ends fall back to their own in-process provider and the user is
told the service needs updating.

## A stale index is a correctness failure, not a performance one

The service must never answer from an index it cannot prove is current. The
per-volume journal registry makes that decision explicit:

| condition                              | status              | what the caller sees               |
|----------------------------------------|---------------------|------------------------------------|
| cursor is inside the journal window    | `healthy`           | results presented as current       |
| journal was recreated                  | `stale`             | volume marked degraded, rebuild    |
| cursor fell behind `FirstUsn` (rollover) | `stale`           | volume marked degraded, rebuild    |
| journal moved backwards                | `stale`             | volume marked degraded, rebuild    |
| volume handle cannot be opened         | `permission-denied` | volume marked degraded, no rebuild |
| volume is not mounted                  | `offline`           | volume marked degraded             |

The UI and `--index-status` surface these states. A degraded volume is reported
as degraded; it is never silently presented as up to date.

## Service crash and volume loss

* A crash of `lce-index-service.exe` cannot crash the desktop, the CLI or the
  MCP server: they hold a pipe, and a closed pipe is an I/O error that triggers
  the in-process fallback.
* One volume failing does not stop another. Each volume has its own worker,
  status and cursor; the supervisor reports per-volume health.
* A worker that cannot read its journal backs off exponentially to 30 seconds
  instead of spinning, and marks only its own volume degraded.
* Shutdown is a flag plus a bounded wait, so the service stops promptly even
  while a journal read is in flight.

## Authentication of the caller

The DACL answers "may this account connect". It does **not** distinguish two
processes belonging to the same user, and this build does not attempt to: the
service serves the interactive user that installed it, and nothing more
sensitive than an index query is exposed. There is no admin-only operation
reachable over the pipe — `RebuildVolume` is the most expensive one, and a
rebuild is a read of the volume the user can already see.

## What is still out of scope

* The service is not a sandbox for a compromised same-user process. Such a
  process can already do everything the service can.
* There is no signature verification of the client. On a machine where the
  user's own account is hostile to itself, this design adds nothing.
* The `Changes` feed is volume-granular (it reports journal advances), not a
  per-file event stream. It is honest about that rather than pretending to
  deliver events it does not have.