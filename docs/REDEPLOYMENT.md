# 开发环境重部署（0.10 Unreleased）

重部署先保存明确的安装请求，再执行并回读结果。已有包的观测版本是清单证据；它本身不构成安装锁文件。当前接入 Homebrew、rustup、uv 和 Windows 可信部署适配器，并输出对应原生格式。

## Homebrew：补齐、滚动和 HEAD

| 策略 | 实际行为 | 完成证据 |
| --- | --- | --- |
| 补齐缺少项（默认） | 生成所选 Brewfile；Bundle install/check 均使用 --no-upgrade | 独立 Bundle 存在性检查 |
| 滚动更新所选包 | 对所选身份安装或更新当前 Homebrew 配方提供的版本 | 执行成功后回读包状态；不将备份版本当成安装目标 |
| HEAD 开发版 | 仅支持 formula，跟随该公式定义的上游 HEAD；通过原生 install/upgrade 请求安装或更新 | 独立读取 Homebrew JSON，验证所选身份确有 HEAD 安装 |

HEAD Brewfile 使用 `brew "name", args: ["HEAD"]`；不解析或执行外来 Ruby Brewfile。原生 HEAD 操作另外处理，避免 Bundle 把已有稳定版的存在误判成 HEAD 已满足。

不自动解除链接、不使用 force/overwrite，也不清理其他包。已有 HEAD 升级前必须证明该 HEAD keg 已链接；稳定版链接、未链接 HEAD 或无法判断的 keg-only opt 状态会在升级前停止，避免升级稳定版后拿旧 HEAD 冒充成功。稳定版与 HEAD 链接冲突、公式没有 HEAD、权限或构建失败均保留为失败。HEAD 验证仅证明对应安装存在，不保证 Shell 的 PATH 已切换、项目能够构建或任意命令正在使用该版本。Homebrew 的 HEAD 也不是任意 Git URL、分支或提交选择器。

滚动请求会随上游变化；当前仓库元数据及缓存决定可解析版本，不保证捕获到网络上此刻最新提交。--no-upgrade 也不阻止安装所需依赖发生变化。各策略都先保存基线和请求，再记录成功、失败或取消；没有自动回滚，取消后子安装器或部分变更可能仍存在。

## Rust：跨 macOS 与 Windows 的原生渠道重部署

Rust 请求独立保存为严格验证的 JSON。接受 `stable`、`beta`、`nightly`、精确 `x.y.z` 和有日期的 `nightly-YYYY-MM-DD`；不接受命令、路径、URL、主机后缀或自定义工具链。捕获已安装工具链时仅保存可移植渠道身份，不把本机架构后缀带到另一台机器。

执行需要本机标准用户位置已有受支持的 rustup 和已安装的官方默认工具链（避免 rustup 首次安装自动改设默认值）；应用不下载 bootstrap 脚本。流程为：读取安装前清单 → 持久保存请求 → 以 minimal profile 和 --no-self-update 安装 → 重新读取已安装清单 → 显式运行该渠道的 rustc --verbose，核对发布版本、提交及日期并记录结果。失败的持久化或回读不会显示为完成。

这不会更改 rustup 默认渠道、Shell 设置或项目目录覆盖。`stable`、`beta` 和 `nightly` 是滚动请求；精确版本与有日期 nightly 固定请求目标，但不是整个开发环境的字节级锁定。minimal profile 不包含现有自定义组件、额外 targets 或项目依赖；工具链清单也不备份 Cargo 安装的所有构建参数和 Git 来源。

## Python 命令行工具：macOS 与 Windows 的 uv 流程

请求只包含经过验证的 PyPI 包名和逐项精确版本；空版本表示滚动请求。读取沿用备份的 `uv tool list --no-config` 解析器。执行前保存所选清单基线与请求，之后只操作所选工具并再次读取身份/版本；结果、失败或取消写入单独证据。读取本机标准 uv 数据与可执行目录，隔离 `UV_*` 和 XDG 自定义路径覆盖，避免清单与部署落到不同工具目录。

精确请求已满足时跳过；其他精确请求使用 `uv tool install --force <name>==<version>`。uv 的强制安装会重建该工具环境，并可能替换执行目录中同名入口，因此 GUI 在执行前提示并要求确认。滚动请求统一使用不带版本约束的 `uv tool install <name>`；这里不用 `uv tool upgrade`，因为 uv 会保留工具原先的版本约束，可能使“滚动最新版”继续受旧约束限制。未选中的工具不卸载或修改。原有入口冲突仍由 uv 拒绝，除精确请求明确授权的 force 行为之外不覆盖其他入口。

执行要求标准位置已有 uv 与系统可用 Python，应用禁用配置文件发现和 uv 自动下载 Python；网络请求采用已启用的应用 Python 源与代理快照。PyPI 包构建后端可能运行第三方代码；中途失败/取消可能留下部分变化，没有通用回滚。pipx 的环境不读取、不卸载，也不自动迁移；清单导入与迁移报告暂缓，避免同时维护两种隔离环境的隐式所有权。Python 项目依赖、虚拟环境及逐字节恢复不在此流程。

## 原生清单输出与成熟项目边界

- `rust-toolchain.toml`：输出所选 Rust 渠道及 minimal profile，由 rustup 按其原生规则读取。应用不直接覆盖现有项目文件。
- uv 工具请求 JSON：可保存/载入选中的 PyPI 命令包与精确/滚动渠道；执行适配器只通过现有 uv 管理所选工具，不运行导入文件中的命令。
- `mise.toml`：仅输出受验证的 Rust `[tools]` 请求，便于加入现有 mise 项目；不含任务、插件、环境、路径、模板或钩子。不执行 mise，不生成或验证 mise.lock。
- `Brewfile`：输出受控所选身份及 HEAD 参数；不把 Homebrew 的已安装版本伪装为版本 pin。
- WinGet 原生 packages 清单：保留现有可信身份导出与逐项部署；本轮未增加 WinGet 任意安装来源或版本恢复。

mise 原生 `mise lock` / `mise install --locked` 可以进一步解析并锁定受支持的工具版本，但须由 mise 实际生成并验证。不能由本应用的观测清单推导出有效锁文件。mise 各后端锁定强度不同，Rust 后端主要记录工具链版本，不锁定项目依赖和所有构建输入。

## 官方契约与验证范围

- [Homebrew Bundle](https://docs.brew.sh/Brew-Bundle-and-Brewfile)、[原生命令](https://docs.brew.sh/Manpage)、[Bundle 行为实现](https://github.com/Homebrew/brew/blob/9811f64e2cccd62fd8a981c1fed5248b9705fea4/Library/Homebrew/bundle/brew.rb)。
- [rustup 渠道](https://rust-lang.github.io/rustup/concepts/channels.html)、[工具链文件与优先级](https://rust-lang.github.io/rustup/overrides.html#the-toolchain-file)、[固定参考源码](https://github.com/rust-lang/rustup/tree/5c8ff3f907c37688d20d9e7038a2f826042cd763)。
- [uv 工具安装与渠道](https://docs.astral.sh/uv/guides/tools/)、[uv tool install/upgrade 参数](https://docs.astral.sh/uv/reference/cli/#uv-tool-install)。uv upgrade 保留已保存约束；force install 会重建选中环境并替换同名入口。
- [mise Rust 后端](https://mise.jdx.dev/lang/rust.html)、[版本请求](https://mise.jdx.dev/dev-tools/versions.html)、[锁文件](https://mise.jdx.dev/dev-tools/mise-lock.html)。

来源、许可证与实际接入范围见 [ENVIRONMENT_SOURCES.md](ENVIRONMENT_SOURCES.md)。本地回归、目标平台编译和真实机器安装验收分别报告；可生成清单或通过测试并不代表已经安装或完成真实渠道切换。
