# 双仓轻量架构升级交付记录（2026-10-13）

本轮保持两个仓库独立；无跨仓 crate、统一核心、数据库、常驻服务或新 GUI。
复用既有 Provider、Tree-sitter、Git、Verification、SARIF、Fleet 和 tracing。

## 变更与架构差异

| 仓库 / 阶段 | 之前 | 本轮改变 | PR |
|---|---|---|---|
| LCE 查询与索引 | 短词/路径条件可触发整表扫描；常见 postings 在构建时大量分配 | 逐条件选择候选源、按基数规划；提前丢弃高频 postings；借用候选切片；仅保留目录路径缓存；修复长 prefix 漏检 | https://github.com/styayur/local-context-engine/pull/19 |
| CodeForge Fleet | 部分非通过状态可跨过必需门禁；任务/PR 身份不稳定 | 必需门禁要求明确 Passed；结构化 Task ID、确定性分支、PR 查询失败时拒绝新建；任务与证据 tracing 关联 | https://github.com/styayur/codeforge-engine/pull/21 |
| CodeForge 上下文 | 无统一按需源码关系提取 | 有界、只读 Tree-sitter 符号/候选调用图，Git 变更反向影响遍历、测试提示、带位置与内容哈希的源码片段 | https://github.com/styayur/codeforge-engine/pull/22 |
| CodeForge 外部分析 | SARIF 字段存在非标准 snake_case；无显式路径证据入口 | 标准 SARIF 兼容旧格式；分析路径关联；显式本地 CodeQL Adapter；限制输入/输出/超时；修复继承管道无限等待 | https://github.com/styayur/codeforge-engine/pull/24 |
| CodeForge 修复精度 | 长度规则可将整个复合条件误替换 | 仅匹配直接 AST 操作数；未解析类型的建议不获得 safe-fix 权限；补充幂等性与精确范围测试 | https://github.com/styayur/codeforge-engine/pull/25 |
| CodeForge 发布准备 | Rust/桌面 0.2.1 | 0.2.2 版本对齐及发布说明 | https://github.com/styayur/codeforge-engine/pull/23 |

## 性能与正确性

LCE：Windows release、固定 seed、100 万记录；每组 3 次预热、40 个样本；限额 2000 的计时与完整有序差分分开。

| 指标 | 升级前 | 升级后 |
|---|---:|---:|
| 索引构建 | 11934.31 ms | 8908.72 ms |
| 构建进程峰值工作集 | 1,005,248,512 B | 258,981,888 B |
| 估算保留索引 | 75,796,186 B | 75,796,186 B |
| `ext:pdf re` 平均 / P95 | 30.042 / 34.343 ms | 7.428 / 9.388 ms |
| `ext:pdf report` 平均 / P95 | 89.360 / 99.741 ms | 20.953 / 23.142 ms |
| `prefix:design report` 平均 / P95 | 87.849 / 93.390 ms | 19.886 / 21.900 ms |
| 常见词 `report` 平均 / P95 | 8.336 / 9.611 ms | 8.472 / 9.867 ms |
| 实际 localsearch.exe | 2,346,496 B | 2,349,568 B |
| 完整有序差分 | — | 8/8 一致 |

峰值内存下降约 74.2%，构建时间下降约 25.4%；保留索引估算不变。常见词回退没有明显加速，不作加速承诺。
LCE Cargo.lock 共 492 个包，其中外部包 475 个，数量不变。

CodeForge：默认 `scan fixtures/python --json`，3 次预热、30 个样本。原始基线平均/P95 为 3239.81/4373.10 ms；相邻复测基线为 2745.89/3530.67 ms；功能阶段为 2699.55/3208.55 ms。路径工具发现和本机负载影响较大，不归因为架构带来的加速。输出摘要一致。
实际 CLI 从 15,139,328 B 到 15,563,264 B，增加 423,936 B（2.80%）。六轮交替测量未复现显著工作集回归；完整读数见 benchmarks/UPGRADE.md。Cargo.lock 共 516 个包，其中外部包 501 个，未增加 Rust 依赖。前端 source-map-js 从 1.2.1 修补至 1.2.2，无新增包。

## 验证与边界

本地运行原始基线测试、修改后的 workspace 测试/clippy、百万数据差分、索引变更/持久化一致性、Unicode/长 prefix/限额排序测试。CodeForge 补充稳定任务与 PR、必需门禁状态矩阵、图歧义/循环/预算/过期源码、SARIF 标准及旧格式、不可信修复、未知 URI base、失败进程、畸形输出及继承管道测试。真实 CLI 冒烟完成。

每个 PR 继续运行原有 CI、CodeForge Regression Gate 与安全检查；失败不合并，也未放宽 Baseline。Release 仅针对检查通过的提交发布。

证据分级：调用关系和测试定位是启发式；源码片段是语法提取；SARIF 是外部静态分析；编译/测试需要实际命令成功；上述任一项都不自动构成语义等价证明。现有事务、回滚、Baseline、权限和 MFT/USN 边界保留。

CodeQL 未安装，使用编译出的本地替身验证协议及故障路径，未声称完成真实 CodeQL 查询集兼容验证。适配器不是 OS 沙箱：不会自动安装或默认运行；用户显式运行的 CLI 可能修改数据库/联网；超时终止直接子进程且限制管道等待，不保证回收全部后代。
图不解析类型、导入、动态分派、宏和被删除符号；partial/歧义显式输出；相关测试建议不代替完整 Verification。

## 许可证和后续路线

只移植思想和接口，未复制框架源码。Renovate/Repowise 的 AGPL 边界、zvec-grep 的 Apache-2.0、jevgrep/Jeview 的 MIT 均已核查。CodeQL CLI 使用和分发限制独立于查询源码许可证；不捆绑 CLI 或查询包，由用户提供符合资格的本地安装。

后续按实测收益推进：真实项目 CodeQL 兼容矩阵与进程树隔离；模块/导入解析改善关系图精度；仅在保留 postings 成为内存主因时评估 delta/varint；全文/语义通过显式启用的 Provider 扩展，继续避开默认热路径。

## 合并与发布状态

LCE #19，CodeForge #21/#22/#23/#24/#25 均在各自的所有 CI、Regression Gate 及适用安全检查通过后合并。两仓主工作区已快进至 main 且无未提交源码。

- LCE main：1bb29900a36880ae26dd91af73274741ca3292d0；v0.2.0 指向该提交。
- CodeForge main：cb390cee1e2cb8baa5b4d73daeb773e1e97b582d。
- CodeForge v0.2.2：4a0c96e83ad08dcd7b89f6d56f1620e5b9efd256；包含 Fleet、安全规则与版本对齐，保留 prerelease 标记。#22/#24 的可选能力在 main，未纳入本次补丁包。
- LCE 打包：https://github.com/styayur/local-context-engine/actions/runs/38396083419
- CodeForge 打包：https://github.com/styayur/codeforge-engine/actions/runs/38396195845

合并后的 CodeForge main 已再次通过完整 workspace 测试及 all-targets/all-features clippy，并完成 release 构建和 context/import-sarif CLI 冒烟。最终 main 的 CLI 为 15,565,312 B（相对基线 +425,984 B，约 +2.81%）；前表的阶段测量为 15,563,264 B，差异来自最后的规则与版本集成。

发布已完成，发布流水线与最终 main CI 均成功：

- [LCE v0.2.0](https://github.com/styayur/local-context-engine/releases/tag/v0.2.0)：Windows 便携包、安装程序及 SHA256SUMS。两份二进制产物均已实际下载并验证 SHA-256。
- [CodeForge v0.2.2 prerelease](https://github.com/styayur/codeforge-engine/releases/tag/v0.2.2)：Windows EXE/MSI、Linux AppImage/DEB 及 checksums。四份产物的清单摘要均与 GitHub 上传摘要一致；已修正清单文件名为实际下载名称，并复核发布后的清单摘要。

CodeForge 现有打包流程对带空格文件名生成清单，而 GitHub 上传时会转换成点号；本次 Release 已修正，后续应把文件名规范化移入打包流程，避免再次人工修正。
两仓主工作区均为 main，无未提交改动。
