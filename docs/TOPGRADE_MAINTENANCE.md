# Topgrade 更新中心

「环境更新」是 DevEnv Studio 单窗口中的 Apple Silicon macOS 页面。默认没有选中项。检查工具和安装记录后，选择 GitHub CLI Skills 或 Helm 仓库，审核并明确确认。重新执行需要重新检查。完整本机配置、Homebrew HEAD-sync、MAS 和需要交互的步骤使用页面提供的终端命令。配置迁移包仍是独立的备份和提取入口。

Studio 使用绝对路径启动独立的 Topgrade。审核绑定二进制和受管脚本、配置及工具身份。受限入口只运行已审核的两个验证器，使用独立的运行请求和结果流。运行记录、有限长度日志和错误保存到已有 Studio 数据目录下的 `maintenance/runs`。普通终端入口、Studio 的相应 Mac 写操作和引擎切换使用同一个维护锁。其他工具直接运行的命令由其各自锁保护。

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
