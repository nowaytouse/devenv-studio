# 开发环境混合工作区的来源与实际集成

目标为同一个 Rust 原生应用中的 macOS 与 Windows 开发环境备份、选择、配置审核和结果检查。下表区分实际接入与设计参考，不把链接或功能计数当成集成。

| 成熟项目 | 固定参考版本 | 许可证 | 当前实际使用 |
| --- | --- | --- | --- |
| [Homebrew/brew](https://github.com/Homebrew/brew/tree/9811f64e2cccd62fd8a981c1fed5248b9705fea4) | 9811f64e2cccd62fd8a981c1fed5248b9705fea4 | BSD-2-Clause | Homebrew formula/cask 清单采集；选择生成 Brewfile；原生 Bundle 配置及独立 check |
| [microsoft/winget-cli](https://github.com/microsoft/winget-cli/tree/bc1e9ff8b982798ab346b61416b76ae738038e57) | bc1e9ff8b982798ab346b61416b76ae738038e57 | MIT | 现有 Windows 可信软件部署；将已审核方案导出原生 packages schema 2.0 清单 |
| [jdx/mise](https://github.com/jdx/mise/tree/d2e9ff8b10a98571740042aa2d2321df1b4b1817) | d2e9ff8b10a98571740042aa2d2321df1b4b1817 | MIT | 设计参考：工具身份与观测版本分离；尚未执行 mise 配置、trust、任务或插件 |
| [twpayne/chezmoi](https://github.com/twpayne/chezmoi/tree/da3406f7aa2a114322334578639812684016d352) | da3406f7aa2a114322334578639812684016d352 | MIT | 设计参考：先检查变化再应用；尚未导入 dotfiles、模板、秘密或钩子 |

来源核对日期：2026-10-23。Homebrew Bundle 已位于 Homebrew/brew；不依赖已归档旧 homebrew-bundle 仓库，不重新分发这四个项目的源代码或二进制。表中许可证是上游自身许可证；本项目许可证保持独立。此前 WinUtil、Sophia Script、Win11Debloat 目录来源与旧 LTSC 工具映射继续保留，参见 INTEGRATIONS.md 与 THIRD_PARTY_NOTICES.md。

## 接入契约

- [Homebrew Bundle 官方文档](https://docs.brew.sh/Brew-Bundle-and-Brewfile)与[命令文档](https://docs.brew.sh/Manpage#bundle-subcommand)：仅从有效身份生成 brew/cask 行，不解析或执行用户提供的任意 Ruby Brewfile。执行时另建私有临时清单，先 install --no-upgrade，再 check --no-upgrade。不会 cleanup、uninstall、zap、force 或从导入文件加载配置钩子。安装工具本身及其依赖仍会运行发布者代码。Homebrew 与本地运行时必须已可用。
- --no-upgrade 只阻止常规主动升级；安装所需依赖仍可能升级或变更。观测版本不是锁文件，不保证二进制、项目构建、Shell 或 CLI 可用性。取消或失败后需要重新检查环境，没有自动回滚。
- [WinGet import 官方文档](https://learn.microsoft.com/windows/package-manager/winget/import)：输出固定官方 winget SourceDetails 和 PackageIdentifier，不带 Version、InitialOverrideArguments、InitialCustomSwitches、认证或任意来源。导出仅含所选 WinGet 应用；Cargo/uv/Pip/NPM/扩展仍在应用档案中。执行继续走逐项可信适配器，不盲目运行导入文件。
- Windows 清单采集不调用安装、升级、winget 来源刷新或网络搜索。Pip 仅记录一个选定标准 CPython 解释器可见的分发元数据；不是所有解释器或虚拟环境备份。
- mise/chezmoi 的任务、插件、模板、秘密与 Shell 钩子形成另一个信任边界；当前只作为有明确出处的设计参考，不宣称已接入执行。

## 尚未覆盖

原生 Mac 配置目前是 Homebrew formula/cask；Cargo、uv、NPM、VS Code 独立来源可备份和向 Windows 可信任务映射，尚未通过 Mac 配置器安装。备份不复制项目、配置文件、二进制、缓存、密钥或虚拟环境；Windows 软件全集与 WinGet 安装清单采集未实现。Windows 配置执行仍使用已支持的 LTSC 目标与运行时门禁，不推断所有 Windows 版本兼容。
