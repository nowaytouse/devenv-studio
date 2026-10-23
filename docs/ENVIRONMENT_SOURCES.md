# 开发环境混合工作区的来源与实际集成

目标为同一个 Rust 原生应用中的 macOS 与 Windows 开发环境备份、选择、配置审核和结果检查。下表区分实际接入与设计参考，不把链接或功能计数当成集成。

| 成熟项目 | 固定参考版本 | 许可证 | 当前实际使用 |
| --- | --- | --- | --- |
| [Homebrew/brew](https://github.com/Homebrew/brew/tree/9811f64e2cccd62fd8a981c1fed5248b9705fea4) | 9811f64e2cccd62fd8a981c1fed5248b9705fea4 | BSD-2-Clause | formula/cask 清单采集、Brewfile、Bundle 补齐；所选身份的滚动更新与 formula HEAD 原生安装及回读 |
| [microsoft/winget-cli](https://github.com/microsoft/winget-cli/tree/bc1e9ff8b982798ab346b61416b76ae738038e57) | bc1e9ff8b982798ab346b61416b76ae738038e57 | MIT | 现有 Windows 可信软件部署；将已审核方案导出原生 packages schema 2.0 清单 |
| [rust-lang/rustup](https://github.com/rust-lang/rustup/tree/5c8ff3f907c37688d20d9e7038a2f826042cd763) | 5c8ff3f907c37688d20d9e7038a2f826042cd763 | MIT OR Apache-2.0 | 可移植 Rust 渠道清单、严格请求、原生重部署与指定渠道 rustc 回读；输出 rust-toolchain.toml |
| [astral-sh/uv](https://github.com/astral-sh/uv/tree/e96aa6932bb860f3b950ddb46d1646d45e09315d) | e96aa6932bb860f3b950ddb46d1646d45e09315d (`0.13.5`) | MIT OR Apache-2.0 | 跨平台 uv tool inventory、逐项 PyPI 请求、精确/滚动重部署与包版本回读；不捆绑 uv |
| [pypa/pipx](https://github.com/pypa/pipx/tree/9a49f1957fad930860189421f2cd29d5a5604fd4) | 9a49f1957fad930860189421f2cd29d5a5604fd4 | MIT | 作为 uv 的同类方案评估；不保留第二个 Python 工具安装后端或自动导入流程 |
| [jdx/mise](https://github.com/jdx/mise/tree/d2e9ff8b10a98571740042aa2d2321df1b4b1817) | d2e9ff8b10a98571740042aa2d2321df1b4b1817 | MIT | 使用受审查的 core runtime 清单、渠道解析、跨平台 mise.lock、--locked 安装与回读；不导入 mise 配置、任务、插件或钩子；Rust 保留专用 rustup 流程 |
| [twpayne/chezmoi](https://github.com/twpayne/chezmoi/tree/da3406f7aa2a114322334578639812684016d352) | da3406f7aa2a114322334578639812684016d352 | MIT | 设计参考：先检查变化再应用；尚未导入 dotfiles、模板、秘密或钩子 |

来源核对日期：2026-10-24。Homebrew Bundle 已位于 Homebrew/brew；不依赖已归档旧 homebrew-bundle 仓库，不重新分发上述项目的源代码或二进制。rustup 与 uv 的许可证通过固定提交中的项目元数据和上游许可证核对；pipx 作为替代方案核对 MIT 许可证。表中许可证是上游自身许可证；本项目许可证保持独立。此前 WinUtil、Sophia Script、Win11Debloat 目录来源与旧 LTSC 工具映射继续保留，参见 INTEGRATIONS.md 与 THIRD_PARTY_NOTICES.md。

## 接入契约

- [Homebrew Bundle 官方文档](https://docs.brew.sh/Brew-Bundle-and-Brewfile)与[命令文档](https://docs.brew.sh/Manpage#bundle-subcommand)：仅从有效身份生成 brew/cask 行，不解析或执行用户提供的任意 Ruby Brewfile。默认补齐另建私有临时清单，先 install --no-upgrade，再 check --no-upgrade。滚动与 formula HEAD 使用单独审核的原生安装策略和回读，详见 [REDEPLOYMENT.md](REDEPLOYMENT.md)。不会 cleanup、uninstall、zap、force 或从导入文件加载配置钩子。安装工具本身及其依赖仍会运行发布者代码。Homebrew 与本地运行时必须已可用。
- --no-upgrade 只阻止常规主动升级；安装所需依赖仍可能升级或变更。观测版本不是锁文件，不保证二进制、项目构建、Shell 或 CLI 可用性。取消或失败后需要重新检查环境，没有自动回滚。
- [WinGet import 官方文档](https://learn.microsoft.com/windows/package-manager/winget/import)：输出固定官方 winget SourceDetails 和 PackageIdentifier，不带 Version、InitialOverrideArguments、InitialCustomSwitches、认证或任意来源。导出仅含所选 WinGet 应用；Cargo/uv/Pip/NPM/扩展仍在应用档案中。执行继续走逐项可信适配器，不盲目运行导入文件。
- Windows 清单采集不调用安装、升级、winget 来源刷新或网络搜索。Pip 仅记录一个选定标准 CPython 解释器可见的分发元数据；不是所有解释器或虚拟环境备份。
- [rustup 官方文档](https://rust-lang.github.io/rustup/concepts/channels.html)：接受稳定、beta、nightly、版本号及日期 nightly 请求。原生 minimal 安装不更改默认工具链；安装后重新检查清单并显式调用所选渠道编译器，记录发布与提交信息。精确请求不代表整个项目或环境已锁定。
- [uv tools 官方指南](https://docs.astral.sh/uv/guides/tools/)与[命令参考](https://docs.astral.sh/uv/reference/cli/#uv-tool-install)：只接管独立 `uv tool` 命令；用户逐项选择 PyPI 包身份、固定版本或滚动版。精确版本冲突时使用 `uv tool install --force` 重建该工具环境，可能覆盖其他包同名入口，所以执行前明确确认。清单读取沿用备份同一解析器；安装前后分别记录/核对所选身份和版本。
- 配置执行使用显式 Python 软件源与代理快照，禁用项目/用户 uv 配置和自动 Python 下载；不会清理未选择工具。应用需要本机已有 uv 与可用 Python。uv 安装的软件包可能运行构建后端代码；没有自动回滚，也不验证项目依赖、命令运行兼容性或字节级可复现性。
- [pipx 官方命令实现](https://github.com/pypa/pipx/blob/9a49f1957fad930860189421f2cd29d5a5604fd4/src/pipx/main.py)保留独立 Python 应用环境管理能力。本项目本轮选择 uv 作为唯一 Python CLI 执行器；pipx 不被卸载、不被启动，也不自动转换其环境。用户可在新请求中逐项录入 PyPI 名称与版本；pipx 清单解析和专用迁移报告待单独设计。
- [mise core tools](https://mise.jdx.dev/core-tools.html)、[CLI lock](https://mise.jdx.dev/cli/lock.html)与[Windows 支持说明](https://mise.jdx.dev/faq.html#does-mise-work-on-windows)：仅管理 Bun、Deno、.NET、Go、Java、Node、Python、Ruby、Zig；逐项选择精确版本/前缀或 latest、stable、LTS、beta、nightly 请求。渠道是否可用由各 core runtime 决定，解析失败时不会开始安装。运行前隔离配置与插件/任务/钩子，在私有临时清单中生成 macOS arm64/x64 与 Windows x64 锁文件，随后按 locked 安装并回读实际版本。安装写入标准 per-user mise 数据目录，不更改 Shell activation、全局或项目配置；导出的已解析 mise.toml 与 mise.lock 配套。锁文件只提供各 core 后端可提供的 URL/校验信息，不锁项目依赖或完整构建输入。Rust 继续走 rustup；任意 Git URL、提交与分支仍由来源专属工作流处理。
- [twpayne/chezmoi](https://github.com/twpayne/chezmoi/tree/da3406f7aa2a114322334578639812684016d352) 继续作为设计参考；不导入 dotfiles、模板、秘密或钩子。

## 尚未覆盖

原生 Mac 配置接入 Homebrew formula/cask，Rust 工具链有独立跨平台流程；Cargo、uv、NPM、VS Code 独立来源可备份和向 Windows 可信任务映射，尚未通过 Mac 配置器安装。任意 Git URL、分支或提交重部署尚未接入；Homebrew HEAD 仅跟随公式定义，Rust beta/nightly 走 rustup 官方渠道。备份不复制项目、配置文件、二进制、缓存、密钥或虚拟环境；Windows 软件全集与 WinGet 安装清单采集未实现。Windows 配置执行仍使用已支持的 LTSC 目标与运行时门禁，不推断所有 Windows 版本兼容。
