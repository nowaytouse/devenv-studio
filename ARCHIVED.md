# LTSC Workspace 退役与保留说明

后续功能开发和维护统一进入 [DevEnv Studio](https://github.com/nowaytouse/devenv-studio)，覆盖 macOS 与 Windows 开发环境备份、配置和重部署。新仓库当前为私有；旧仓库的公开访问不赋予新仓库访问权限。旧仓库保留为历史依据，不再作为独立维护项目。

## 冻结依据

- 整合前的旧 main 为 [`a8523abbe08147b4d5de8746f26d99d056057ab0`](https://github.com/nowaytouse/LTSC_Tools_Backup/tree/a8523abbe08147b4d5de8746f26d99d056057ab0)。本次退役说明是其后的文档变更，不改写该历史提交。
- 退役准备时，旧仓库没有 Git 标签或 GitHub Release。现有提交和文件保留，不为退役创建新的产品版本。
- 根目录 `ltsc_setup_gui.exe` 未被替换。它仍来自源提交 `3fd48acec93650f885bd235073d9bb3abb8fe486` 的历史 CI 产物，SHA-256 为 `b1f783bad5c0ba3f8ee8e54e309bd05558e1a61d4060be17e8514ed46186505c`。后续源码能力不能算作这个 EXE 已具备的能力；详见 [既有验证记录](Docs/READINESS.md)。
- 本地旧源码目录、内置资源、清单及原有文件保留。GitHub 归档不意味着删除本地文件，也不意味着从设备移除已安装软件。

## 未提交工作已经另行保留

退役前已有 `Cargo.toml` 与 `Cargo.lock` 未提交修改。两文件原样保留在原路径，并在仓库外的私有目录 `.devenv-retirement-20261023T131732Z` 保存原始副本和 `Cargo-worktree.patch`；`SHA256SUMS` 覆盖副本、补丁和完整 Git bundle。Git bundle 使用 `--all` 保存并通过 `git bundle verify` 校验；该本地备份包含工具检查点引用，不上传到仓库或发行资产。

| 原始文件 | 退役准备时的 SHA-256 |
| --- | --- |
| `Cargo.toml` | `5a0420f9614424d4ae024aa72e56c748d13ff809f3c1858f2ee81faed056e63a` |
| `Cargo.lock` | `435e1c684550adb38d78590350b1adc0d78033f983ec51af7b04c7cebf28cb40` |

这些修改是保留的工作进度，不是已经通过验证的最终依赖组合。尤其 `windows-collections 0.100.0` 与旧 `windows 0.62.2` 的投影兼容性已有历史失败记录，不能直接据此构建并宣称可用。退役没有通过 stash、reset、restore 或删除文件消除这些改动。

## 整合与安全边界

新项目吸收旧 Rust 后端时保留来源，接入新工作台的任务、取消和执行互斥机制。Windows 操作仍限于 Windows；macOS 的可见入口不构成已执行或已验证 Windows 操作的证据。迁移对应关系及剩余范围由新项目的 `docs/LEGACY_MERGE.md` 记录；保留历史不等于所有功能行为完全一致。

旧目录没有独立的整仓许可证声明。所有者自有源码的授权不覆盖其中的第三方资产：`cjk_font.ttf` 内嵌元数据声明 LXGW WenKai Lite / SIL OFL 1.1，部分 skills 标注 Apache-2.0，而部分插件和 skills 缺少完整许可依据。新程序不因归档而自动复制或执行这些资源，也不将其整体重标为 MIT。旧插件 hook 中存在个人机器路径，必须先完成隐私与可移植性审查；不能把资源成功释放当作插件可运行。

旧回滚账本没有新项目的认证契约。`RollbackLatest` 不直接迁为新 GUI 的可执行入口，也不将旧账本导入新认证恢复区。需要历史恢复时，应使用可验证的旧程序与相应旧记录，在可恢复的测试设备上独立核对；不要把新程序的恢复按钮当作旧账本的兼容承诺。

## 归档顺序与验收限制

先保留本地工作和 Git 历史，再完成并推送 DevEnv Studio 的整合提交，最后推送旧仓库的退役说明并设置 GitHub 只读归档。执行归档时保留旧仓库地址、源码提交、文件和历史产物；不重写历史，不覆盖旧 EXE，不自动上传本地清单或备份。

本文为退役准备和保留记录；它本身不证明 GitHub 归档操作已经完成，也不证明 Windows/LTSC 实机部署、安装、回滚或新 CI 已通过。历史验证仍以 [Docs/READINESS.md](Docs/READINESS.md) 中明确对应的源码和产物为准。
