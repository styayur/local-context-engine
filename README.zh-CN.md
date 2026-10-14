# Local Context Engine

**面向 Windows 的本地优先上下文发现引擎。** 用一套可解释查询查找文件、应用、进程、服务和窗口。

[English](README.md) · [下载 v0.2.0](https://github.com/styayur/local-context-engine/releases/tag/v0.2.0) · [文档目录](docs/README.md)

当前已发布版本为 **v0.2.0**。v1.0.0 仍是未发布候选说明，不代表已有对应安装包或新的兼容性承诺。

## 解决什么问题

同一项工作可能对应文件、应用和运行中的进程。LCE 将这些本地来源放进同一个查询入口，并展示候选选择与排序依据。文件索引保存名称、路径和元数据，不读取全文用于检索；日常搜索不需要账户、模型服务或网络。

## 能力与成熟度

| 状态 | 能力 | 边界 |
|---|---|---|
| Stable | Query DSL、中英文规则式查询编译 | 检索不依赖模型 |
| Stable | Query Planner、扩展名/prefix/trigram 索引 | 候选仍需验证；宽泛条件可能扫描索引 |
| Stable | 文件、应用、进程、服务、窗口 Providers | 元数据受 Windows 权限与后端能力限制 |
| Stable | CLI、本地 stdio MCP、解释与健康状态 | 查询、快照、设置、使用记录相同才具有确定性排序 |
| Experimental | 可选的特权 MFT/USN 服务、跨环境桌面验收 | 已实现并有测试，部署环境仍需验证 |
| Future | 全文、语义检索、模型查询编译器 | 不属于默认能力，也不承诺交付时间 |

Stable 表示已有实现和回归覆盖，不表示通过了所有硬件、文件系统或桌面环境的认证。

## 架构

```text
用户 → CLI / Desktop / MCP → Search Core → Providers
                                             ↓ 文件分支
                                  Query Planner → Accelerators
                                             ↓
                                   候选验证 → 排序与解释
```

Planner 位于文件 Provider 内部；其他 Provider 读取各自的系统快照。详见[架构](docs/ARCHITECTURE.md)。

## 性能

Windows、Rust 1.98.1、百万条合成数据的成对测试：

| 指标 | 优化前 | 优化后 |
|---|---:|---:|
| 加速索引构建 | 11.93 秒 | 8.91 秒 |
| 构建进程峰值工作集 | 1005.25 MB | 258.98 MB |
| `ext:pdf report` 平均 / P95 | 89.36 / 99.74 ms | 20.95 / 23.14 ms |
| 常见词 `report` 平均 / P95 | 8.34 / 9.61 ms | 8.47 / 9.87 ms |
| 估算保留加速索引 | 75.80 MB | 75.80 MB |

内存收益是构建峰值下降，不能解读为保留索引变小。八组完整有序结果与线性参考一致。完整方法、环境信息缺口、样本数和非加速案例见[性能报告](docs/BENCHMARKS.md)。

## 安装

从 [v0.2.0 Release](https://github.com/styayur/local-context-engine/releases/tag/v0.2.0) 下载 Windows x64 便携包或安装程序，对照 `SHA256SUMS.txt` 校验。解压后运行 `localsearch.exe`，可将目录加入 PATH。桌面需要 WebView2，应在实际机器上做启动与搜索验收。

源码构建使用当前 stable Rust MSVC 与 Visual Studio C++ Build Tools：

```powershell
git clone https://github.com/styayur/local-context-engine.git
cd local-context-engine
git checkout v0.2.0
cargo build --release --locked
.\target\release\localsearch.exe --help
```

无需安装特权服务即可使用普通用户目录扫描。共享机器启用 MFT 前，请先阅读[权限边界](docs/SECURITY.md#mft-and-permissions)。

## 使用

```powershell
localsearch --providers
localsearch --backend scan "type:file ext:pdf"
localsearch --explain "ext:pdf report"
localsearch "prefix:read"
localsearch "type:process python"
localsearch --index-status
```

查询退出码：`0` 有结果，`1` 查询有效但无匹配，`2` 操作无法完成。相对时间、实时快照和使用记录变化会影响结果。详见[查询引擎](docs/QUERY_ENGINE.md)、[DSL](docs/DSL.md) 与 [MCP](docs/MCP.md)。

## 安全与隐私

默认检索不上传数据、不索引全文。路径缓存、元数据和使用记录仍是敏感本地数据。打开结果由 Windows 处理；结束进程需要确认。可选特权服务可能暴露普通目录遍历无法列出的文件名，但不会授予内容读取权限。详见[安全模型](docs/SECURITY.md)与[漏洞报告](SECURITY.md)。

## 开发

[开发指南](docs/DEVELOPMENT.md)说明构建和检查；[贡献指南](CONTRIBUTING.md)说明扩展原则；[路线](docs/ROADMAP.md)区分发布门槛与未来能力；[变更记录](CHANGELOG.md)面向用户描述变化。

仓库许可证为 [MPL-2.0](LICENSE)。桌面 package 元数据仍有待对齐的许可证差异，见[许可证说明](THIRD_PARTY_LICENSES.md)。本仓独立运行，不依赖 CodeForge Engine 的运行时或 Rust crate。
