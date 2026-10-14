# Indexing

## Stored data

The file store keeps names and metadata and reconstructs full paths from parent
relationships. Accelerators retain extension postings, prefix keys and selected
full-path trigrams. File contents, embeddings and a vector database are not part
of this index. Settings and optional bounded usage data are also stored locally;
protect the cache as sensitive metadata. See [Security](SECURITY.md).

## Backends

On first run, an empty `indexedDrives` setting selects every fixed local volume,
with a default ceiling of 1,000,000 entries including directories. Permission
failures and that ceiling can make the index incomplete. Settings live in
`%LOCALAPPDATA%\LocalContextEngine\settings.json`, alongside local cache/usage
data. To restrict the local index, set `indexedDrives` to drive letters such as
`["C"]` in the existing settings file (preserving its other fields), restart the
client and rebuild the index. Desktop settings expose the same configuration.
`path:` filters query results; they do not restrict collection. The CLI has no
`--root` option. Optional service-owned indexing has its own deployment scope;
client settings must not be treated as a service permission boundary.

| Backend | Behavior | Limitations |
|---|---|---|
| Directory scan | Walk readable directories under the current account | Filesystem I/O cost; inaccessible paths omitted; links/junctions not followed |
| NTFS MFT/USN | Enumerate names and replay journal mutations | Elevated volume access; NTFS-specific; raw metadata may expose names beyond normal directory ACL visibility |
| Optional index service | Own privileged volume handles and per-volume workers | Experimental deployment boundary; not installed by a normal CLI build |

`auto`, `scan` and `mft-usn` are backend choices. Availability and privilege
influence selection; inspect `--index-status` rather than assuming MFT was used.
MFT file-size metadata is limited; a size-dependent query is not equivalent to
reading file contents or stat-ing every file.

## Journal consistency

Volume identity uses a volume GUID with serial/mount metadata, not only a drive
letter. Each volume has its own journal ID, window and cursor. A missing cursor,
recreated journal or out-of-window cursor requires a rebuild. Offline or denied
volumes are reported separately. Workers use bounded journal reads and backoff;
mutation batches update the store and accelerators together. `Changes` reports
volume journal advances, not a complete per-file event API.

The cache uses versioned MessagePack with named fields (current schema 3).
Incompatible state is rebuilt rather than guessed at. A stale/degraded state is
visible health information, not a guarantee of current results.

## Resource use

Common trigram postings are pruned during construction; dropped-trigram state
survives updates and persistence. Prefix capacity is compacted, and path caching
retains directory strings. These reduce transient allocations without a new
compressed posting format. Retained index estimates and process working set
measure different things; [Benchmarks](BENCHMARKS.md) reports both.

```powershell
localsearch --index-status
localsearch --rebuild-index
localsearch --update-index
localsearch --service-status
```

Rebuild/update modify local index state, not source files. Follow the
[service security boundary](SECURITY.md#mft-and-permissions) before installing
an elevated service.
