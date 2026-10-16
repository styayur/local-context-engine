<div align="right">
  <strong>简体中文</strong> | <a href="./README.md">English</a>
</div>





# Local Context Engine

离线优先的本地系统搜索。

自然语言输入 → 确定性查询 → Rust 搜索内核 → 结果。

[下载](https://github.com/styayur/local-context-engine/releases/latest) · [架构](docs/ARCHITECTURE.md) · [查询语法](docs/DSL.md) · [MCP](docs/MCP.md) · [性能](docs/BENCHMARKS.md)

[![CI](https://github.com/styayur/local-context-engine/actions/workflows/ci.yml/badge.svg)](https://github.com/styayur/local-context-engine/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/styayur/local-context-engine?label=release&color=orange)](https://github.com/styayur/local-context-engine/releases)
[![license: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue)](LICENSE)



---

## 这是什么

Local Context Engine 是一个 Windows 本地搜索工具：一个输入框，同时搜索**文件与文件夹**、**应用**、**正在运行的进程**、**服务**和**窗口**，并且可以直接打开它们。

输入 `vscode`，你在同一个列表里看到应用、进程和匹配的文件。输入 `正在运行的 python` 或 `running python processes`，得到 Python 进程。输入 `最近的 pdf` 或 `recent PDFs`，得到按修改时间排序的 PDF。

它建立在三个判断之上：

1. **检索必须确定性。** 同一个查询，每次都返回同样的结果、同样的顺序。热路径上不出现模型、embedding 或向量数据库。
2. **理解是编译问题。** 自然语言被**编译**成结构化查询——`最近的 pdf` 变成 `type:file ext:pdf sort:modified-desc`——然后交给 Rust 内核执行。
3. **离线是默认状态，而不是一种模式。** 没有账号、没有遥测，仓库里没有一行网络代码。

## 为什么做这个

Windows 上的桌面搜索要么是全量内容索引器（建索引要几分钟），要么是只能看见快捷方式的启动器。而所谓“AI 搜索”的常见答案通常是“把所有东西都做 embedding 再碰运气”——慢、不确定、无法解释，而且悄悄把文件名发到了别处。

这个项目选择相反的路线：

> 如果一定要有 AI，它是**查询编译器**。检索始终是确定性的本地操作。

## 与传统桌面搜索的区别

|                             | 传统索引器      | 启动器      | Local Context Engine                       |
|-----------------------------|-----------------|-------------|--------------------------------------------|
| 首次可用时间                | 建索引数分钟    | 即时        | 实时数据即时可用，文件索引一次扫描          |
| 索引文件**内容**            | 是              | 否          | 否——只索引名称、路径与元数据                |
| 覆盖进程 / 服务 / 窗口      | 罕见            | 否          | 四类全部覆盖                                |
| 查询方式                    | 单一输入框      | 单一输入框  | 输入框 + 公开的 DSL + MCP 接口              |
| 排序确定性                  | 通常不确定      | 通常不确定  | 始终确定，`--explain` 可给出评分依据        |
| AI 在检索热路径上           | 有时            | 有时        | 从不                                        |
| 是否需要网络                | 经常需要        | 不需要      | 从不需要                                    |
| 内存占用                    | 数百 MB         | 数十 MB     | 一百万个条目约 70 MB                        |

## 架构

```text
             User / AI
                 │
                 ▼
          Query Compiler            ← 未来唯一允许接入 AI 的位置
                 │
                 ▼
           SearchQuery
                 │
                 ▼
           Search Core
                 │
     ┌───────────┼───────────┐
     ▼           ▼           ▼
   Files      Processes     Apps
     │           │           │
  MFT/USN       Win32      Registry
```

工作区的划分让“这段代码该放哪”不再需要讨论：

```text
local-context-engine/
├─ apps/desktop/            Tauri 2 + React + TypeScript 桌面外壳
├─ crates/
│  ├─ search-core/          实体 / 查询 / 结果模型、搜索引擎、模糊匹配
│  ├─ query-dsl/            DSL 解析器与规则式自然语言编译器
│  ├─ ranking/              启发式评分 + 有界的本地使用记录
│  ├─ windows-files/        目录扫描与 NTFS MFT + USN 两种索引后端
│  ├─ windows-processes/    实时进程快照
│  ├─ windows-apps/         App Paths、开始菜单、PATH、WindowsApps
│  ├─ windows-services/     只读的服务快照
│  ├─ windows-windows/      顶层窗口快照
│  ├─ search-daemon/        三个前端共用的组合根
│  ├─ search-cli/           localsearch.exe
│  ├─ search-mcp/           localsearch-mcp.exe（stdio 上的 MCP）
│  └─ benchmarks/           10k / 100k / 100 万条目的 Criterion 基准
├─ docs/
└─ tests/                   工作区级集成测试
```

完整说明（包括为什么 `AI != Search Engine`、Provider 边界、内存账目）见
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

## 查询语法

```text
chrome
type:file rust
type:file ext:rs
type:file ext:pdf modified:<24h
type:process python
type:process name:node
type:app vscode
type:service state:running
type:window github
path:projects ext:toml
drive:C ext:exe sort:size-desc
path:"My Projects" ext:toml
size:>10mb sort:size-desc limit:20
```

可用键：`type`、`ext`、`path`、`name`、`drive`、`modified`、`created`、`size`、
`state`、`pid`、`user`、`visible`、`sort`、`limit`。解析**永不失败**——无法识别的键会作为普通文本参与搜索，所以
`localsearch "C:\Users\me"` 是一次搜索而不是语法错误。

完整的时间与体积运算符说明见 [docs/DSL.md](docs/DSL.md)。

## MCP

`localsearch-mcp.exe` 通过 stdio 把同一个内核暴露给任何 MCP 客户端：

```jsonc
{
  "mcpServers": {
    "localsearch": { "command": "C:\\path\\to\\localsearch-mcp.exe" }
  }
}
```

| 工具                | 作用                                |
|---------------------|-------------------------------------|
| `search_system`     | 跨全部数据源搜索                    |
| `search_files`      | 文件与文件夹                        |
| `search_processes`  | 正在运行的进程                      |
| `search_apps`       | 已安装的应用                        |
| `search_services`   | Windows 服务                        |
| `search_windows`    | 打开的顶层窗口                      |
| `compile_query`     | 只展示编译结果，不执行搜索          |
| `index_status`      | 后端、条目数、内存、各数据源状态    |
| `dsl_reference`     | 键表与示例                          |
| `record_selection`  | 记住选择，下次提升排序              |
| `terminate_process` | 危险操作，必须传 `confirm: true`    |

详见 [docs/MCP.md](docs/MCP.md)。

## 命令行

```bash
localsearch chrome
localsearch "type:file ext:rs"
localsearch "正在运行的 python"
localsearch --type process node
localsearch --json vscode
localsearch --explain "昨天修改的 rust 文件"
localsearch --index-status
localsearch --rebuild-index
localsearch --update-index
localsearch --providers
localsearch --list-usage
localsearch --reset-usage
```

退出码是契约的一部分：`0` 有结果，`1` 查询合法但无匹配，`2` 命令未能完成。
`--json` 会把单个 JSON 文档写到 stdout，诊断信息始终走 stderr。

## 桌面端

```bash
cd apps/desktop
npm install
npm run tauri:dev      # 开发模式，热重载
npm run tauri:build    # 生成 NSIS 安装包
```

* 搜索框自动聚焦，键盘优先：`↑`/`↓` 导航，`Enter` 打开，`Shift+Enter` 打开右键菜单，
  `Tab` 切换类型筛选，`Esc` 清空或关闭，`Ctrl+,` 打开设置。
* 类型筛选标签带实时计数、匹配高亮、按实体类型区分的右键菜单，状态栏显示编译结果与耗时。
* **简体中文与 English**，切换无需重启。所有文案——错误、空状态、提示、右键菜单——都在
  `src/i18n/{zh-CN,en-US}.json` 中，组件里没有任何硬编码文本。
* 设置涵盖语言、主题、结果上限、模糊匹配、使用频率加权、索引磁盘、索引后端、全局快捷键，以及完整的索引面板。
* 唯一的破坏性操作“结束进程”需要确认，对话框默认焦点在“取消”；Rust 侧在没有显式
  `confirmed: true` 时同样会拒绝执行。

## 构建

需要：Rust（stable，MSVC 工具链）、安装 “使用 C++ 的桌面开发” 工作负载的 Visual Studio Build Tools、Node 20+，以及 WebView2（Windows 11 自带）。

```bash
git clone https://github.com/styayur/local-context-engine
cd local-context-engine

# 命令行与 MCP 服务
cargo build --release

# 桌面端与安装包
cd apps/desktop
npm install
npm run tauri:build
```

产物位于 `target/release/`：

```text
target/release/localsearch.exe
target/release/localsearch-mcp.exe
```

## 开发

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test

cd apps/desktop
npm run lint
npm run typecheck
npm run build
```

基准测试：

```bash
cargo bench -p lce-benchmarks --bench file_search
cargo bench -p lce-benchmarks --bench ranking
```

目录约定、如何新增数据源、如何新增语言，见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 安全

* 文件搜索是**只读**的。
* 没有 `exec`、没有 `eval`、不会用查询拼出 shell 命令。
* 持久化状态只有本项目自己写的 MessagePack 索引和两个 JSON 文件，不会把不受信任的数据当作可执行内容反序列化。
* 路径一律按不透明 Unicode 字符串处理，并针对空格、简体中文、超长路径编写了测试；从不跟随 junction 与符号链接。
* 结束进程在边界两侧都需要确认。
* 本版本对服务严格只读。

完整威胁模型见 [SECURITY.md](SECURITY.md)。

## 隐私

> **你的本地索引永远不会离开这台机器。**

* 没有遥测、没有分析、没有崩溃上报、没有更新检查。
* 不需要账号、不需要登录、没有云服务。
* 查询历史可选、有上限、保存在本地
  `%LOCALAPPDATA%\LocalContextEngine\usage.json`，可在设置里或通过
  `localsearch --reset-usage` 清空。
* 搜索内核的依赖树中没有 HTTP 客户端。

## 性能

下面都是 Criterion 在合成数据集上的实测值，不是估算。完整表格、测试机器、方法与注意事项见
[docs/BENCHMARKS.md](docs/BENCHMARKS.md)。

| 项目 | 索引 100 万条目时 |
|---|---:|
| 查询编译（`最近的 pdf`、`recent PDFs`、纯文本、DSL） | 1.4 – 11.6 µs |
| 命中候选上限的常见输入场景 | **约 8 ms** |
| 必须遍历全部记录的搜索 | **约 62 ms** |
| 同上，且对每条记录做模糊匹配 | **约 76 ms** |
| 对 4 000 个候选排序打分 | **约 2.8 ms** |
| 完整热路径：遍历索引 → 收集候选 → 排序 | **约 10 ms** |
| 构建内存索引 | 316 ms（约 320 万条目/秒） |
| 100 万条目的内存占用 | 约 69 MB |

候选数量上限（`CANDIDATE_CAP = 4 000`）是 100 万条目索引不会比 1 万条目慢五十倍的原因；
真正会随规模增长的是那些必须遍历全部记录的情况。

## 路线图

* 通过 `AppModel\Repository` 支持 MSIX/Appx 应用发现，而不仅依赖开始菜单快捷方式。
* 解析 `$DATA` 属性，让 MFT 后端也能给出文件大小。
* 由事件驱动索引更新，而不是按需重放变更日志。
* 可选的无边框命令面板窗口。
* 可选的 AI 查询编译器，实现同一个 `QueryCompiler` trait——默认关闭，且永远不在检索热路径上。

## 许可

[MIT](LICENSE)。与 Listary 及其它启动器**没有任何关联**，也不包含其代码、资源或界面。
本项目只借鉴了“本地索引应当足够快”这一公开的设计思路。