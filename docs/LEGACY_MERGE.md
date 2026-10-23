# LTSC Workspace 后端整合与退役

DevEnv Studio 是后续唯一维护入口。旧 [LTSC_Tools_Backup](https://github.com/nowaytouse/LTSC_Tools_Backup) 保留为历史来源，不删除旧本地目录、旧源码、旧产物或未提交工作。整合从旧 main [`a8523abbe08147b4d5de8746f26d99d056057ab0`](https://github.com/nowaytouse/LTSC_Tools_Backup/tree/a8523abbe08147b4d5de8746f26d99d056057ab0) 开始；这不是通过替换项目名字来宣称功能已经合并。

## 源码进入同一 Rust 工作区

旧后端的七个模块迁入工作区 crate `studio-legacy`：

| 旧源文件 | 保留职责 |
| --- | --- |
| `src/config.rs` | 旧配置模型、工具映射与输入校验 |
| `src/inventory.rs` | 旧 Mac 工具清单、受限来源和导入校验 |
| `src/installer.rs` | 旧 Windows 部署任务与结构化执行事件 |
| `src/platform.rs` | 旧 Windows 能力与系统参数处理 |
| `src/rollback.rs` | 旧原值记录模型；不开放未认证的通用恢复入口 |
| `src/utils.rs` | 原有超时、取消、有界输出和持久化错误处理 |
| `src/winget_bootstrap.rs` | 原有 WinGet 引导与产物校验 |

旧 GUI 不作为另一个程序嵌套启动。新 GUI 使用同一个后台 worker、取消信号和执行互斥锁调度迁入的后端；旧任务与其他系统写入不能同时启动。所有变更仍需明确选择与确认，真实事件决定状态；失败、超时、取消和输出失败不能被转换成成功。Windows 执行能力与 macOS 预览分开，macOS 展示旧能力不表示已检查或修改 Windows 主机。

这七个模块保留原有信任边界和错误传播，并适配新工作区的校验与线程模型。TRIM 按 NTFS / ReFS 分别记录明确的 0/1 原值，遇到未设、重复或未知结果拒绝写入，执行后重新查询；命令参数和 0/1 含义对照 [Microsoft fsutil behavior 文档](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/fsutil-behavior)，真实设备行为仍需独立验收。旧代码中的所有者源码经明确授权适配；Windows 投影依赖必须使用兼容类型家族，不把保留的未提交依赖组合当作已验证配置。整合后的具体功能、缺口和验证证据由本次代码、测试及 [VALIDATION.md](VALIDATION.md) 共同确定；[LEGACY_PARITY.md](LEGACY_PARITY.md) 作为此前比较检查点保留。

## 不自动迁入的内容

- 旧 `src/assets/skills`、`src/assets/plugins` 与其 hook/MCP 配置不自动复制或执行。部分资源缺少完整许可证，有旧插件记录个人 Mac 绝对路径；先完成来源、隐私、许可证和 Windows 可用性审查，再决定是否单独接入。
- 旧 `cjk_font.ttf` 的元数据声明 LXGW WenKai Lite / SIL OFL 1.1。新应用继续使用系统字体；如日后引入该字体，必须保留完整许可证和 copyright，不能改标为 MIT。
- 旧个人 Mac 工具快照不因整合而成为新应用默认采集结果，也不上传用户当前备份。需要的可信映射和公开样例应与个人观测分开审核。
- 旧 `RollbackLatest` 使用未认证历史，不直接映射为可执行任务。旧账本不进入新认证恢复目录；新恢复机制只处理符合各自来源、版本和认证契约的记录。
- 根目录旧 EXE 不复制成 DevEnv Studio，也不以旧二进制的 CI 证据证明新程序可用。旧 EXE 继续按其原始提交和摘要保留。

## Git 历史与未提交工作的保留

主项目在完成源码适配提交后，以保留当前工作树的历史合并，将旧 main `a8523abbe08147b4d5de8746f26d99d056057ab0` 及其 41 个可达提交并入 main ancestry。功能由 `studio-legacy` 与主界面实际接线提供；历史合并不覆盖新项目源码，也不导入本地代理检查点引用。不得把整个旧目录无差别复制为新的 MIT 资产集合，不重写旧历史。交付时以 ancestry 检查与远端提交确认该历史已保留。

退役准备已在旧仓库之外创建 `.devenv-retirement-20261023T131732Z`：完整 `git bundle --all`、原始 `Cargo.toml` / `Cargo.lock`、`Cargo-worktree.patch` 和 `SHA256SUMS`。bundle 通过 `git bundle verify`，四项摘要全部匹配；其内包含 agents/codex 检查点引用，仅留在本地私有备份。远端历史导入仅选择经过审核的旧 main 可达历史，不发布本地检查点 bundle。

| 保留对象 | SHA-256 |
| --- | --- |
| 原始 `Cargo.toml` | `5a0420f9614424d4ae024aa72e56c748d13ff809f3c1858f2ee81faed056e63a` |
| 原始 `Cargo.lock` | `435e1c684550adb38d78590350b1adc0d78033f983ec51af7b04c7cebf28cb40` |
| `Cargo-worktree.patch` | `0d9c3aab0381565d5f63c3e8840ab1b4844044574308ef08c06b6e7679f7b1fb` |
| `LTSC_Tools_Backup-all.bundle` | `31e5940c037a83c94bf811ffe2cc8da74efdecff195bed02d9313e7d6f7ff5f3` |

原文件保持原路径。它们是已有工作进度，不要求先提交到旧 main 才能归档；尤其旧 `windows-collections 0.100.0` 与 `windows 0.62.2` 的组合有已知兼容失败记录。不得为清理工作区丢弃修改、强推或把未验证组合标为最终可用。

## 旧项目的退役顺序

1. 保留并验证 Git bundle、未提交工作和摘要。
2. 完成主项目实际整合、必要检查和精确提交推送；不提升当前 0.10.0 开发基线、不创建发行标签。
3. 推送旧项目 README 重定向和 `ARCHIVED.md`，注明新仓库当前私有，公开用户可能没有访问权限。
4. 最后设置旧 GitHub 仓库只读归档，保留旧地址、历史文件和产物；旧本地目录继续保留。

旧根目录 `ltsc_setup_gui.exe` 来自 `3fd48acec93650f885bd235073d9bb3abb8fe486`，SHA-256 为 `b1f783bad5c0ba3f8ee8e54e309bd05558e1a61d4060be17e8514ed46186505c`，本次不替换。退役准备时旧仓库没有标签或 GitHub Release；归档不创建新发行。本文描述整合与保留契约，不替代实际提交、归档状态、Windows/LTSC 设备验收或签名产物证据。
