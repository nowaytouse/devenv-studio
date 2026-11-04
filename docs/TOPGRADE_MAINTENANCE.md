# 跨平台更新中心

| 平台 | 清单与更新 | 配置和记录 |
| --- | --- | --- |
| Apple Silicon macOS | 查看 Homebrew 已安装清单，审核后执行受控 Topgrade 步骤，其他步骤在终端预演 | 受管 profile 事务、引擎回退和 Studio 更新记录 |
| Windows | 读取 WinGet 匹配的已安装清单，为明确的社区源软件提供单包核对与升级命令 | 进入已有的系统配置、软件部署和维护审核，查看 Studio 部署记录 |

Windows 读取使用 WinGet 的版本化导出，可能访问已配置的源来匹配软件。清单显示导出范围和可获得的版本，升级可用性由终端的核对命令确定。只有官方社区源且 ID 唯一的行提供可复制命令。升级在终端处理，Studio 的部署记录涵盖由 Studio 执行的任务。Windows LTSC 2021 和 2024 仍按各自的目标和主机检查控制配置写入。 [WinGet 导出](https://learn.microsoft.com/en-us/windows/package-manager/winget/export) · [WinGet 核对与升级](https://learn.microsoft.com/en-us/windows/package-manager/winget/upgrade)

## macOS Topgrade

当前组件已同步上游 [v17.15.0](https://github.com/topgrade-rs/topgrade/releases/tag/v17.15.0)，上游提交为 `155a490a35261382def837b572c306dd55590d39`。Studio 的 macOS 受管步骤和本机 profile 随组件继续维护。

「更新中心」从已核验的 Topgrade 引擎读取步骤目录。GitHub CLI 技能和扩展、Helm、TLDR、Homebrew 链接审计和 CLI 工具版本可以在 GUI 内审核后执行。默认零选中，重跑前需重新检查。其余引擎步骤及本机 profile 的定制任务有可搜索的终端入口。预演读取正常 Topgrade 配置，前后置命令及配置中的 `misc.only` 也会参与；以预演结果核对实际范围。配置迁移包仍是独立的备份和提取入口。

「Homebrew 已安装」读取本机 Formula 与 Cask，提供搜索、已安装版本、链接、`opt` 及固定版本细节，并展示读取命令。清单使用 Homebrew 的本地 JSON 列表，读取时关闭自动更新。单包安装、升级和卸载可由 [Homebrew 官方应用](https://github.com/Homebrew/BrewUI)处理，Studio 的 Homebrew 维护继续遵循本机 Topgrade profile。

「更新任务」的审核视图显示每项的命令或运行方式。任务结束后进入「执行记录」，可查看结果和本地输出，也可以独立刷新历史。

Studio 使用绝对路径启动独立的 Topgrade。审核绑定二进制、受管配置和选中工具的身份。受限入口用 Rust 子进程执行白名单步骤，通过独立请求和事件记录汇报。运行记录和有界日志位于现有 Studio 数据目录的 `maintenance/runs`。普通终端入口、Studio 的相应 Mac 写操作和引擎切换共用维护锁。

当前 Rust 迁移的范围：

| 功能 | 正常 profile | GUI |
| --- | --- | --- |
| GitHub CLI Skills、Helm | 原生验证器保留归属、共享锁和缓存检查 | 可审核执行 |
| Homebrew 链接审计、CLI 版本报告 | Rust 审计与版本探测 | 可审核执行 |
| GitHub CLI 扩展、TLDR | Topgrade 自身步骤 | 可审核执行 |
| gcloud、MAS、Xcode Beta 选择 | Rust 保留重试、核验或回退边界 | 终端入口 |
| Homebrew HEAD-sync | Rust 保留兼容事务日志、固定 LLVM、成对完整 SHA 校验和恢复 | 终端入口 |
| SQLite CLI、Python 3.15 SQLite | Rust 共享源码版本与更新锁，保留事务和运行时验证 | 终端入口 |
| Gradle Nightly | Rust 核对下载和安全解包，验证运行时后切换启动器 | 终端入口 |
| Krita Next、BetterDisplay、TourBox Beta、Xcode Beta 安装 | Rust 核对来源、包身份和安装回读，保留备份或厂商安装机制 | 终端入口 |
| 项目滚动依赖 | Rust 识别项目和依赖面，写入变更前备份与历史，执行受管工具并验证结果 | 终端入口 |
| NPX、设计包和 Cursor 覆盖检查 | Rust 管理归属、镜像、锁和可恢复激活，使用已固定版本的 NPX 更新工具 | 终端入口 |
| npm 捆绑依赖安全 | Rust 验证两份包目录、公告范围及交换事务 | 终端入口 |

旧的 Shell 和 Python 文件随 profile 保留，供兼容入口和回退使用。受管 Topgrade 流程使用 Rust 任务，按需调用 Homebrew、GitHub CLI、npm、Gradle 和厂商安装工具。Python 3.15 SQLite 的 ABI 验证继续通过目标解释器执行。

项目滚动更新保留既有依赖面和每仓库策略。npm 检查使用本机已安装的 ncu，禁用依赖安装脚本。Gradle 依赖报告固定使用 0.67.0 插件；wrapper 仅更新校验通过的版本与分发属性，保留原启动脚本和 JAR。共享版本引用影响范围无法证明时，只记录发现的候选。旧独立滚动更新命令和预览应用手动启动选项仍可作为兼容入口。

取消在当前步骤结束后生效。超时或无法证明子进程结束时，保留中断记录，暂停新的维护操作。先检查任务和现存变更，再在终端运行已安装 Topgrade 的 `--studio-acknowledge-interruption`。如果中断记录缺少可核验的进程组信息，先人工确认没有残余进程并核验更新器状态，再保留该记录作恢复依据。

## 本机构建与切换

源代码来自保留完整历史的 Topgrade 组件，使用独立 Cargo 配置及原许可证。维护者从干净、审核过的 Studio checkout 操作：

```sh
devenv-studio topgrade-engine plan /absolute/studio-checkout FULL_STUDIO_SHA
devenv-studio topgrade-engine prepare /absolute/studio-checkout FULL_STUDIO_SHA
devenv-studio topgrade-engine install /absolute/studio-checkout FULL_STUDIO_SHA
```

`prepare` 构建并核验固定的组件树，保存候选引擎及摘要。 `install` 使用 Studio 的 Rust 安装器验证资产、应用并回读 profile，然后原子切换到准备好的二进制。安装器兼容原有 v1 收据和事务日志。相同组件树复用引擎。软件更新使用已部署的引擎，引擎维护由上述命令显式启动。

原有 Homebrew HEAD-sync 日志仍可由 Rust 入口恢复。收到 TERM 或 HUP 时，更新器持锁等待正在运行的子进程，随后恢复两份公式并以相应的退出状态结束。如果只有父进程被强制杀死，请先确认原来的 Homebrew 和构建子进程均已退出，再运行恢复。

回退同一次部署时，使用相同 checkout 和 SHA，把 `install` 换成 `rollback`。命令从激活记录找到原二进制和独立的 profile 回滚事务，若较新的修改使回滚不安全则停止。若过程报告 pending 激活，请保留文件、启动器和 profile 事务，核对实际生效版本后再恢复。私人记录在 `~/.local/share/topgrade-rustup-head` 和 `~/.local/state/topgrade-macos-profile`。

Studio 预览包包含 Studio 自己的可执行文件。引擎由本机单独构建。仓库内保留 Topgrade 源代码、第三方声明及 GPL 许可；分发引擎时须一并处理对应源码与构建材料。
