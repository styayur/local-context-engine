# Providers

Providers collect normalized entities; the shared ranker scores them. They do
not contain UI logic. The actual interface is
[EntityProvider](../crates/search-core/src/provider.rs); consult source before
implementing it rather than copying an obsolete trait declaration.

| Provider | Source | Important limits |
|---|---|---|
| Files/directories | Directory scan or NTFS MFT/USN index | Metadata only; health, permission and backend limits apply |
| Applications | App Paths, Start Menu, PATH and WindowsApps discovery | Discovery is not a complete package inventory |
| Processes | Live Windows process snapshots | Protected processes and optional account lookup restrict metadata |
| Services | Windows service snapshots | Read-only inspection; no service-management command |
| Windows | Top-level window snapshots | Visibility/ownership filters; not accessibility-tree or browser-tab search |

```powershell
localsearch --providers
localsearch "type:app vscode"
localsearch "type:process python"
localsearch "type:service state:running"
localsearch "type:window github"
```

A snapshot may change immediately after a query. Failed or unavailable providers
report health; an empty result does not establish that the operating system has
no such object. Opening a result and confirmed process termination are separate
user actions, described in [Security](SECURITY.md).

To extend a domain, implement the existing provider boundary and register it in
SearchService. Keep filtering/ranking/presentation responsibilities separate and
add deterministic fixture/error tests. The repository has no provider dependency
on another project. Full-text and semantic providers remain future extensions.
