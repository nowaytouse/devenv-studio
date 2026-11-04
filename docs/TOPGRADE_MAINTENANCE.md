# Topgrade 更新中心

「更新中心」从已核验的 Topgrade 引擎读取步骤目录。GitHub CLI 技能和扩展、Helm、TLDR、Homebrew 链接审计和 CLI 工具版本可以在 GUI 内审核后执行。默认零选中，重跑前需重新检查。其余引擎步骤及本机 profile 的定制任务有可搜索的终端入口。预演读取正常 Topgrade 配置，前后置命令及配置中的 `misc.only` 也会参与；以预演结果核对实际范围。配置迁移包仍是独立的备份和提取入口。

Studio 使用绝对路径启动独立的 Topgrade。审核绑定二进制、受管配置和选中工具的身份。受限入口用 Rust 子进程执行白名单步骤，通过独立请求和事件记录汇报。运行记录和有界日志位于现有 Studio 数据目录的 `maintenance/runs`。普通终端入口、Studio 的相应 Mac 写操作和引擎切换共用维护锁。

当前 Rust 迁移的范围：

| 功能 | 正常 profile | GUI |
| --- | --- | --- |
| GitHub CLI Skills、Helm | 原生验证器保留归属、共享锁和缓存检查 | 可审核执行 |
| Homebrew 链接审计、CLI 版本报告 | Rust 审计与版本探测 | 可审核执行 |
| GitHub CLI 扩展、TLDR | Topgrade 自身步骤 | 可审核执行 |
| gcloud、MAS、Xcode Beta 选择 | Rust 保留重试、核验或回退边界 | 终端入口 |
| HEAD-sync、SQLite、Preview/Beta 应用安装、Gradle、项目滚动依赖、NPX/设计包和 npm 安全修复 | 继续使用受管脚本 | 终端入口 |

旧验证器文件仍随 profile 保存，用于兼容及回退。新引擎的六个受控步骤不调用这些 Python 或 Shell 验证器。Profile 安装器和上表剩余任务继续依赖原有运行时。

取消在当前步骤结束后生效。超时或无法证明子进程结束时，保留中断记录，暂停新的维护操作。先检查任务和现存变更，再在终端运行已安装 Topgrade 的 `--studio-acknowledge-interruption`。如果中断记录缺少可核验的进程组信息，先人工确认没有残余进程并核验更新器状态，再保留该记录作恢复依据。

## 本机构建与切换

源代码来自保留完整历史的 Topgrade 组件，使用独立 Cargo 配置、原许可证和 Python 3.15 profile。首版依赖本机已有的 Rustup Nightly、Homebrew Python 3.15 及对应更新工具。维护者从干净、审核过的 Studio checkout 操作：

```sh
devenv-studio topgrade-engine plan /absolute/studio-checkout FULL_STUDIO_SHA
devenv-studio topgrade-engine prepare /absolute/studio-checkout FULL_STUDIO_SHA
devenv-studio topgrade-engine install /absolute/studio-checkout FULL_STUDIO_SHA
```

`prepare` 构建并核验固定的组件树，保存候选引擎及摘要。 `install` 验证已有启动器、应用并回读 profile，然后原子切换到准备好的二进制。相同组件树复用引擎。软件更新使用已部署的引擎，引擎维护由上述命令显式启动。旧 Homebrew HEAD-sync journal 和工具链固定策略仍由原脚本处理。

回退同一次部署时，使用相同 checkout 和 SHA，把 `install` 换成 `rollback`。命令从激活记录找到原二进制和独立的 profile 回滚事务，若较新的修改使回滚不安全则停止。若过程报告 pending 激活，请保留文件、启动器和 profile 事务，核对实际生效版本后再恢复。私人记录在 `~/.local/share/topgrade-rustup-head` 和 `~/.local/state/topgrade-macos-profile`。

Studio 预览包包含 Studio 自己的可执行文件。引擎由本机单独构建。仓库内保留 Topgrade 源代码、第三方声明及 GPL 许可；分发引擎时须一并处理对应源码与构建材料。
