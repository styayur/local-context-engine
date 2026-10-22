# Local Context Engine

用于文件、应用、进程、服务和窗口的 Windows 离线搜索与本地 stdio MCP 服务。

[English](README.md) · [下载 v0.2.0](https://github.com/styayur/local-context-engine/releases/tag/v0.2.0) · [文档目录](docs/README.md)

**阶段：Alpha。** 已实现流程有回归覆盖；独立用户的安装、升级和持续使用验证仍待完成。本指南介绍当前源码。最新正式发布为 v0.2.0，v1.0.0 说明仍属未发布候选，不表示已有对应安装包或兼容性保证。

当前 Rust workspace、桌面包和安装程序配置均声明 0.2.0。版本号与 Release 相同，不代表默认分支的所有改动已进入发布程序。

## 功能

- 通过显式 DSL 或中英文规则式查询编译搜索本地文件名、路径与元数据，日常搜索不需要账户、模型服务或网络。
- 同一服务查询文件/目录、应用、进程、服务和窗口，并展示排序解释、Provider 健康状态和索引信息。
- 提供 CLI、桌面外壳和本地 stdio MCP 适配器。可选的特权 NTFS MFT/USN 服务独立于非特权扫描后端。
- 文件查询先通过加速索引缩小候选，再精确验证；宽泛查询仍可能扫描。确定性排序要求查询、快照、设置与使用记录一致。

```text
CLI / Desktop / MCP → SearchService → Providers
                                      ├─ 文件：Planner → 加速索引 → 验证
                                      └─ 应用 / 进程 / 服务 / 窗口
                                  → 排序与解释
```

Planner 属于文件 Provider，不是每个 Provider 的公共步骤。详见[架构](docs/ARCHITECTURE.md)和[查询引擎](docs/QUERY_ENGINE.md)。已有[性能报告](docs/BENCHMARKS.md)记录合成数据测量及其限制，不提供延迟保证。

## 安装 / 快速开始

### 已发布 Windows 软件包

下载 [Windows x64 便携 ZIP](https://github.com/styayur/local-context-engine/releases/download/v0.2.0/local-context-engine-windows-x64-v0.2.0.zip)或[桌面安装程序](https://github.com/styayur/local-context-engine/releases/download/v0.2.0/local-context-engine-desktop-v0.2.0-setup.exe)，对照 [SHA256SUMS.txt](https://github.com/styayur/local-context-engine/releases/download/v0.2.0/SHA256SUMS.txt) 校验：

```powershell
Get-FileHash <downloaded-file> -Algorithm SHA256
```

首次文件搜索前，请检查[索引范围](docs/INDEXING.md)：`indexedDrives` 为空时，会在条目上限内选择所有固定本地卷。`path:` 仅筛选结果，不限制采集。关闭客户端后，在 `%LOCALAPPDATA%\LocalContextEngine\settings.json` 选择盘符。如果文件不存在，创建该目录和 UTF-8 JSON 文件；下例仅选择 C 盘。已有文件只修改 `indexedDrives`，保留其他设置：

```json
{"indexedDrives": ["C"]}
```

必须使用合法 JSON，文件不可读或格式错误时会回退到默认设置。修改范围后重启客户端；已有索引需重建。备份设置及可选的 `%LOCALAPPDATA%\LocalContextEngine\usage.json` 可保留偏好；索引是可重建的本地元数据。

解压便携包，在 PowerShell 中进入包含 `localsearch.exe` 的目录，运行：

```powershell
.\localsearch.exe --help
.\localsearch.exe --providers
.\localsearch.exe --backend scan "type:file ext:pdf"
.\localsearch.exe --explain "ext:pdf report"
.\localsearch.exe "type:process python"
.\localsearch.exe --index-status
```

扫描后端不需要可选的特权服务。搜索有结果返回 `0`，合法查询无结果返回 `1`，操作无法完成返回 `2`。Provider 能力受 Windows 权限限制，实时结果和相对时间条件随系统变化。详见 [DSL 参考](docs/DSL.md)和[索引指南](docs/INDEXING.md)。

使用桌面包时，运行安装程序，再启动 **Local Context Engine**。它需要 WebView2，并应在实际 Windows 环境验收；构建成功不能证明窗口正常渲染或搜索可用。

### 构建当前源码

使用 Windows x64、当前稳定版 Rust MSVC 工具链和 Visual Studio C++ Build Tools。桌面开发还需要 Node.js 22（CI 版本）、npm 和 WebView2。

```powershell
git clone https://github.com/styayur/local-context-engine.git
cd local-context-engine
cargo build --release --locked
.\target\release\localsearch.exe --help
```

这些命令构建当前默认分支，不是已发布的 v0.2.0 Tag。默认 Cargo 成员包含 CLI 和 MCP 服务，不包含桌面依赖栈；桌面打包与输出路径见[开发说明](docs/DEVELOPMENT.md)。

从源码启动桌面：在 `apps/desktop` 先运行 `npm ci`，再运行 `npm run tauri:dev`；Tauri 会打开原生窗口并启动开发服务器。

### 连接 MCP 客户端

构建后，`target/release/localsearch-mcp.exe` 通过本地 stdio 提供 MCP。在客户端配置中，将示例路径替换为程序的绝对路径：

```json
{
  "mcpServers": {
    "localsearch": {
      "command": "C:\\tools\\lce\\localsearch-mcp.exe"
    }
  }
}
```

`search_system`、`compile_query` 等工具见 [MCP 工具与协议示例](docs/MCP.md)。搜索本身在本地执行，但 MCP 客户端或模型可能按自身隐私设置将结果发往其他服务。

## 测试 / CI / 平台支持

[![CI](https://github.com/styayur/local-context-engine/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/styayur/local-context-engine/actions/workflows/ci.yml)

在 Windows 的仓库根目录运行：

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release --locked
```

桌面检查在 `apps/desktop` 运行：

```sh
npm ci
npm run lint
npm run typecheck
npm run build
```

[CI 工作流](.github/workflows/ci.yml)在 Windows 上执行 Rust 与桌面检查，包括在仓库根目录执行 `cargo check -p local-context-engine-desktop`。[CodeForge regression](.github/workflows/codeforge-regression.yml)是外部开发门禁，不是运行时或 Rust crate 依赖。Markdown 与链接检查见[文档检查](docs/DOCUMENTATION_CHECKS.md)。

## 已知限制

- 运行时后端支持 Windows，不提供 Linux 运行时支持。Cargo 声明 Rust 1.82，但 CI 检查当前稳定版，未验证该最低版本。
- 文件搜索索引名称、路径与元数据，不搜索文件全文。全文、语义检索和模型查询编译器属于未来工作，不是当前能力。
- 特权 MFT/USN 服务与桌面需跨机器验收。此前出现的 WebView 空白不能仅凭构建成功视为已解决。服务 ACL、日志回卷、回退与卸载仍需部署验收。
- 路径缓存、元数据、设置和可选使用记录是敏感本地数据。特权服务可能显示普通目录遍历无法列出的文件名，但不授予文件内容读取权限；在共享机器启用前阅读[安全说明](docs/SECURITY.md)。

## 路线图

发布 v1.0 前需一并更新各软件包版本、验证 Rust 最低版本，并完成全新安装、桌面渲染与服务验收。之后可改善元数据完整性和应用发现；语义或全文扩展必须显式选择启用。[路线图](docs/ROADMAP.md)与[发布准备](docs/RELEASE_PREPARATION.md)区分已实现流程、实验性部署路径和未来工作。

## 贡献

遵循 [CONTRIBUTING.md](CONTRIBUTING.md)和[开发说明](docs/DEVELOPMENT.md)。可复现问题提交 [Issue](https://github.com/styayur/local-context-engine/issues)，漏洞按 [SECURITY.md](SECURITY.md) 报告。维护权限边界与中英文能力说明的一致性。

## 许可证

仓库 [LICENSE](LICENSE) 与 Rust workspace 声明 **MPL-2.0**。桌面包元数据仍写为 MIT，[第三方许可说明](THIRD_PARTY_LICENSES.md)记录了这一待解决的不一致。本次文档变更不修改许可证或包清单。依赖保留各自条款，发布声明的完整性仍需单独审核。
