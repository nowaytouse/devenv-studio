# 版本与发布

DevEnv Studio 覆盖 macOS 与 Windows 开发环境备份、配置、网络方案和重部署；Windows LTSC 系统设置是其中一个独立区域。项目与主命令使用 `devenv-studio`，仓库为 [nowaytouse/devenv-studio](https://github.com/nowaytouse/devenv-studio)。

## 当前开发基线

包版本保留 **0.10.0**。更名和后续改进先记录在 CHANGELOG.md 的 **Unreleased**，不把每轮对话、功能提交、依赖更新或本地预览构建当成一次发布。历史 0.1.0–0.10.0 记录、已有标签和旧产物保留，不回退版本或重写历史。

版本号描述一个发布检查点；源码提交标识区分同一开发基线上的不同构建。未发布的本地构建不能仅凭 `0.10.0` 宣称与旧包完全相同，也不能被称为新的正式发行。macOS 打包首先生成 `DevEnv Studio 0.10.0.app`；同名产物存在时改用 `DevEnv Studio 0.10.0-preview-<UTC>.app`，保留已有包。时间标签区分本地预览构建，不改变包版本，也不表示新发布。

## 何时选择下一个版本

- 当前 `0.x` 阶段，经过审查的一组完整功能里程碑，或有明确兼容性说明的破坏性变更，才进入一次次版本发布。
- 只含兼容性缺陷修复的发布使用修订号；普通提交先累计在 Unreleased。
- 需要提供候选版本时，可在发布检查点使用 `-alpha.N`、`-beta.N` 或 `-rc.N`，不为每次开发构建创建新的预发布版本。
- 文件格式和目录快照各有自己的兼容性版本；产品改名或包版本变化不自动递增它们。

## 发布检查点

只有用户明确要求发布，或约定的发布检查点明确授权时，才选择版本、创建标签或发布 GitHub Release。代码提交、推送、持续集成检查和手动打包各自独立；推送代码不隐含发布授权，打包工作流不自动创建 Release。

发布前审核 Unreleased 的实际范围与兼容性说明，一次性同步工作区包版本和锁文件中的本地包版本，并记录对应源码提交。按变更范围完成仓库规定检查，分别写明本地测试、目标编译、原生界面、设备验收、签名和公证的实际证据；未执行的检查保留为未验证。然后整理该版本变更记录，保留后续 Unreleased 区域。

1. 从干净 checkout 构建。记录完整 Studio 提交、Cargo.lock 摘要、Rust 工具链、目标三元组及 SDK 版本。若交付 Topgrade 引擎，另记录组件树、引擎锁文件、profile 清单、二进制摘要及对应源码与许可材料。
2. 使用 Developer ID 签名 macOS 产物，完成 Apple 公证后装订票据，在干净的目标环境验证签名、Gatekeeper、安装与启动。Windows 安装包使用 Authenticode 和 SHA-256 时间戳，按标准用户场景在两代 LTSC 核验签名、启动及权限提示。 [Apple 公证](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution) · [Microsoft SignTool](https://learn.microsoft.com/en-us/windows/win32/seccrypto/signtool)
3. 在最终签名和封装完成后记录对外产物的 SHA-256。保留已验证上一版及其摘要，在目标设备验证升级和回退。发布记录关联签名验证结果与设备验收记录。

## Windows x64 本地构建

在 PowerShell 7 中进入仓库根目录，运行 `& .\scripts\Enter-DevEnvironment.ps1`，再执行 Cargo 构建。需要已有 Visual Studio 或 Build Tools 的 x64 C++ 工具和 Windows SDK。脚本通过 Visual Studio Installer 查找带 C++ 工具的最新实例（包括预览版），调用该实例的官方开发终端脚本，并核对实际解析到的 `cl.exe`、`link.exe` 和 SDK 文件。发现相对路径、工具链不匹配或缺少 SDK 文件时会恢复当前进程的环境变量并报告原因。初始化只作用于当前 PowerShell，不写用户或系统 PATH。 [Microsoft Developer PowerShell](https://learn.microsoft.com/en-us/visualstudio/ide/reference/command-prompt-powershell?view=visualstudio) · [Microsoft vswhere](https://github.com/microsoft/vswhere/wiki/Find-VC)

### Local Windows x64 build

Open PowerShell 7 in the repository root and run `& .\scripts\Enter-DevEnvironment.ps1` before building with Cargo. Install the x64 C++ tools and Windows SDK through Visual Studio or Build Tools. The script uses Visual Studio Installer to locate the newest qualifying instance, including preview versions. It loads that instance's Developer PowerShell and verifies the resolved `cl.exe`, `link.exe`, and SDK files. A relative path, mismatched toolchain, or missing SDK file restores the original process environment and reports the problem. Initialization affects only the current shell. [Microsoft Developer PowerShell](https://learn.microsoft.com/en-us/visualstudio/ide/reference/command-prompt-powershell?view=visualstudio) · [Microsoft vswhere](https://github.com/microsoft/vswhere/wiki/Find-VC)

## 更名兼容性

`devenv-studio` 是新的主命令；`ltsc-studio` 保留为调用同一实现的兼容入口。源码目录、`studio-*` 内部 crate 名称不必随产品名移动或更名。

旧应用数据目录 `LTSC Studio` 与 bundle identity `io.github.nowaytouse.ltsc-studio` 继续保留，确保已有偏好、网络方案、认证密钥与恢复记录从同一位置读取。档案字段、任务 ID、日志认证域和历史来源标识同样保持稳定。这些名称属于有意保留的兼容性契约；不能用无差别替换清理，否则可能隔离历史或破坏验证。需要迁移时另行设计、审核并验证。
