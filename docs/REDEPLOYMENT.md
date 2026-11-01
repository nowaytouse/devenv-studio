# 开发环境重部署（0.10 Unreleased）

重部署先保存明确的安装请求，再执行并回读结果。已有包的观测版本是清单证据；它本身不构成安装锁文件。当前接入 Homebrew、rustup、mise core、uv、macOS 固定来源 Cargo CLI 工具和 Windows 可信部署适配器，并输出对应原生格式。

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

## Cargo CLI tools：macOS 固定来源重部署

来源仅限八个已审核的 Cargo 包：ripgrep（[BurntSushi/ripgrep](https://github.com/BurntSushi/ripgrep)）、fd-find（[sharkdp/fd](https://github.com/sharkdp/fd)）、bat（[sharkdp/bat](https://github.com/sharkdp/bat)）、eza（[eza-community/eza](https://github.com/eza-community/eza)）、zoxide（[ajeetdsouza/zoxide](https://github.com/ajeetdsouza/zoxide)）、starship（[starship/starship](https://github.com/starship/starship)）、just（[casey/just](https://github.com/casey/just)）和 git-delta（[dandavison/delta](https://github.com/dandavison/delta)）。不接受用户提供的 crate 名、Git URL、分支或提交。

每项可留空版本以跟随审核 Cargo registry 的滚动版本，或输入精确 semver（包括上游发布的 beta 预发布版本）；Git HEAD 选项只读取固定 HTTPS 仓库默认分支，解析一个完整提交 SHA 后以 `--rev` 安装。Cargo home 与子进程工作目录使用私有临时目录，安装根固定为 `~/.cargo`；执行前记录当前安装清单和请求，HEAD 解析结果在任何写入前单独落盘，完成后再次读取清单并核对包身份/精确版本。Git HEAD 报告中的 SHA 证明传给 Cargo 的安装请求，不是从二进制读取的提交指纹。滚动版不构成锁定，构建脚本以当前用户权限执行且没有自动回滚；HEAD 会覆盖同名可执行文件。

Cargo registry 镜像适用于 registry 源和依赖；Git HEAD 保持固定 GitHub HTTPS 地址，仅使用本应用网络方案中的代理设置。Windows 继续使用现有逐项审核的 Cargo 任务。

## NPM global packages：macOS

只从完整且已保存的 Mac NPM 清单中逐项选择全局包。渠道留空时恢复清单记录的顶层版本；填写 `latest`、`beta`、`next`、`nightly` 或该包实际发布的 dist-tag 时，所有选中包共用该标签。只接受清单内的包身份，不接受 Git、URL 或本地路径。安装后在同一 Homebrew prefix 回读全局包清单；精确版本必须相同，dist-tag 流程回读包身份并记录实际版本。

执行使用本机 Homebrew Node.js 与 npm CLI，当前只支持 `/opt/homebrew` 或 `/usr/local` 标准位置。任务使用已启用的应用 NPM registry 与代理配置，但不读取用户/全局 `.npmrc` 或认证信息；需要令牌的私有源目前不可用。npm 安装脚本默认关闭，所以依赖 postinstall 等脚本的包可能无法工作。只核对顶层包版本，不锁定传递依赖、二进制或构建结果；已有正确精确版本会跳过安装。

执行前写入基线、所选请求和网络方案，执行后写入已回读状态与结果。失败或取消会尽力保存部分状态；不会卸载、清理或自动回滚。npm 当前文档说明了全局安装与 dist-tag 行为：[npm install](https://docs.npmjs.com/cli/v11/commands/npm-install/)、[npm dist-tag](https://docs.npmjs.com/cli/v11/commands/npm-dist-tag/)。

## VS Code extensions：macOS

只从完整且已保存的 Mac 清单中选择扩展；请求载入不会恢复执行确认，新增备份会清除旧选择。执行复用与备份采集相同的稳定版 Visual Studio Code 应用内置 Electron / `cli.js`，仅使用 `/Applications` 或 `~/Applications` 下的标准应用包，并安装到默认用户配置文件。该面板不切换 VS Code profile，不导入设置、账号或同步状态。

保留备份版本会请求官方 CLI 支持的 `publisher.extension@version` 精确版；清空版本表示当前 Marketplace 版本。应用安装前展示扩展 ID、目标版本和权限风险，并要求再次确认，然后运行 `--install-extension … --force`；`--force` 避免无人值守时等待 CLI 提示。完成后从相同默认配置文件运行 `--list-extensions --show-versions`，精确版必须匹配请求版本，滚动版必须能读回扩展身份。前后清单、请求、结果及失败状态写入私有运行证据；失败时尽力重新读取部分变更。

扩展代码运行时享有 VS Code 相同的权限。Marketplace 当前版会变化，精确版本也可能在未来不可用；扩展可能自动更新，失败或取消可能留下已安装部分，本流程没有通用回滚。应用自己的代理与软件源方案不控制 VS Code Marketplace 网络；按 VS Code 自身代理设置访问。NPM 包流程则使用应用内选择的 registry 与代理；VS Code Marketplace 继续按 VS Code 自身代理访问。

## mise core：macOS 与 Windows 开发运行时

mise 清单读取使用 mise --no-config ls --installed --json --backend core，只记录运行时身份、已安装版本和清理候选状态，不保存安装路径。用户可选择全部显示的已安装版本（或逐项勾选）并保存为 JSON 重部署备份。界面逐项支持九种经本项目核对的 core runtime：Bun、Deno、.NET、Go、Java、Node.js、Python、Ruby、Zig。请求可用 JSON 保存/载入；精确版本、前缀以及 latest、stable、lts、beta、nightly 先由 mise core 解析为具体版本。特定 runtime 不支持的渠道会在锁定/安装前以错误停止。任意 Git URL、HEAD 提交与分支不适用于所有 core runtime，不伪装成通用能力；Rust 继续由 rustup 面板管理。

执行仅用应用生成的 [tools] 清单，并为 macos-arm64、macos-x64、windows-x64 生成 mise.lock，保存清单、锁文件、安装前/后清单与终态结果。每一步忽略继承的 MISE_* 与 XDG 配置/数据路径，使用临时隔离的 global/system 配置根，停用配置环境变量和 hooks，不运行用户任务或插件；运行时仍安装到 mise 标准 per-user 数据目录。安装后重新读取版本并逐项核对。显式 --locked 要求当前平台已有锁定下载 URL；backend 无法提供时失败关闭，不回退到未锁定安装。

导出请求清单时保留用户渠道；部署完成后导出的已解析 mise.toml 固定本次具体版本，必须与该结果的 mise.lock 一起使用。安装不会改 Shell activation、全局/项目配置或 PATH；用户可审核并将导出的清单和锁文件放入目标项目。mise.lock 对部分 core backend 记录下载 URL 和可用校验值；它不锁项目依赖、操作系统包、构建输入，也不保证字节级复现。失败或取消可能已安装部分版本，没有自动回滚。

## Topgrade 配置与脚本迁移包

- 保存原文，不将完整配置转换为少量白名单开关。自动采集 `~/.config/topgrade.toml`、`~/.config/topgrade/topgrade.toml` 及两种目录布局的 `topgrade.d/*.toml`；外部 include、自定义命令依赖的脚本或数据文件需要明确添加附件。仅支持当前主目录内的普通 UTF-8 文本，不跟随符号链接或执行命令。
- 档案 schema 1 包含相对路径、精确文本和原可执行标记。文件最多 128 项，单文件 256 KiB，总文本 512 KiB，编码档案 1 MiB；拒绝未知字段、重复路径、大小写冲突、路径穿越和链接。采集结束前重新读取源文件，变更则失败。
- 私人档案保存在用户指定的绝对路径，以暂存文件、同步和不覆盖发布后回读。Unix 档案权限 0600。档案不上传，也不随应用分发。
- GUI 载入会清空文件选择与确认；审核原文后逐文件选择并确认还原。CLI 需明确 `--all` 或重复 `--file`。目标必须为不存在的新目录；原文字节回读相同才报告成功，失败清理本次创建的目录。Unix 目录 0700、文件 0600，不恢复执行权限。
- 这是配置与明确附件的迁移，不等于安装或运行环境已经恢复。目标设备需先准备 Topgrade、解释器、JDK、包管理器或其他脚本依赖，并审核旧主目录路径、外部 include 和自定义命令，再自行决定启用。Git HEAD、nightly、beta 渠道如果由原脚本定义，会原样保存；工具二进制、源树、缓存、项目依赖和运行状态不在包中。
- 官方配置实现确认：显式 `--config` 会绕过自动 `topgrade.d` 发现，因此本应用不生成只指向主配置就宣称完整启用的运行命令。XDG_CONFIG_HOME 或 Windows APPDATA 等其他配置根目录若位于当前主目录内，可将配置逐文件作为明确附件采集；本轮自动发现仅覆盖上述主目录布局。
- 参考 [Topgrade 固定版本配置实现](https://github.com/topgrade-rs/topgrade/blob/352e81e9abb7602b4b8aa68f514fac8dc5b453b4/src/config.rs)，仅独立实现数据归档契约，不复制 GPL 引擎或分发其二进制。

### fork 环境与安装来源

2026-10-31 本设备已切换到所有者 [nowaytouse/topgrade fork](https://github.com/nowaytouse/topgrade/tree/0a2969bf976ce124b813a7064d17119a68cc25ac)，固定 revision `0a2969bf976ce124b813a7064d17119a68cc25ac`，版本仍为 17.14.1。当前二进制、Cargo 安装来源记录和本地源码 revision 已一致核对；二进制指纹、个人配置、脚本原文及归档只保存在本地私有快照，不成为应用默认配置。

fork 的 [macOS profile](https://github.com/nowaytouse/topgrade/blob/0a2969bf976ce124b813a7064d17119a68cc25ac/profiles/macos/README.md) 包含 `[helm].repository_updater`、`[github_cli].skills_updater` 等专用配置和相应验证脚本。迁移器原样保存这些文本；目标设备必须核对 fork 来源、固定提交和先决工具，仅凭版本号不能判断兼容。34 文件文本包与 210 文件资源附件覆盖固定 profile 的 241 项及三个本地配置/安装记录，已逐项核验；资源附件超过文本包边界，因此单独私有保存。归档并不代表二进制或全部工具链已重部署。

本仓库使用 fork 已支持的 `.topgrade-dependencies.json`（严格内容 `{"allow_incompatible": false}`）参与兼容依赖滚动更新。现用更新器会对本仓库执行 `cargo update`，保留清单约束及 Windows 绑定的配套精确版本；跨大版本升级仍需单独审查并协调 API。这是本仓库策略，不修改其他项目或全局更新器。

2026-11-01 再次只读核对：活动 fork、完整 revision、二进制指纹和两份安装记录仍与上述来源一致；34 份文本、241 项已安装 profile 及 210 份资源载荷均仍匹配原私有快照，因此保留原包并单独增加复核记录。资源 tar 另含 210 份 macOS AppleDouble 元数据侧文件，单独校验，不计入 profile 载荷数量。Topgrade 源码工作区另有未提交的锁文件改动，本轮未改动或提交该仓库，不能把源码工作区状态当作已安装二进制的构建证明。

## 原生清单输出与成熟项目边界

- `rust-toolchain.toml`：输出所选 Rust 渠道及 minimal profile，由 rustup 按其原生规则读取。应用不直接覆盖现有项目文件。
- uv 工具请求 JSON：可保存/载入选中的 PyPI 命令包与精确/滚动渠道；执行适配器只通过现有 uv 管理所选工具，不运行导入文件中的命令。
- Rust 的 mise.toml 输出仍保留。mise core 运行时另有严格 JSON 请求与 GUI 重部署；导出的请求/已解析 mise.toml 只含受支持的 core: runtime，锁文件由 mise 实际生成并验证，不导入项目配置、任务、插件、环境或钩子。
- `Brewfile`：输出受控所选身份及 HEAD 参数；不把 Homebrew 的已安装版本伪装为版本 pin。
- WinGet 原生 packages 清单：支持可信身份导出，以及在迁移工作台审核 schema 2.0 导出 JSON。只有文件声明的固定社区来源与唯一可信目录身份才产生候选；Store、自定义源、重复/未知身份和附加安装要求留为说明。版本只作元数据，不提供精确版本、渠道、范围、安装参数或任意来源恢复；执行继续使用原有可信任务，不把文件传给 winget import。

mise core 流程实际调用 mise 生成跨平台锁文件并使用 install --locked；不从观测清单伪造锁条目。锁强度因 backend 而异，虽有 core archive 下载 URL/校验值仍不锁项目依赖、操作系统包与全部构建输入。

## 官方契约与验证范围

- [Homebrew Bundle](https://docs.brew.sh/Brew-Bundle-and-Brewfile)、[原生命令](https://docs.brew.sh/Manpage)、[Bundle 行为实现](https://github.com/Homebrew/brew/blob/9811f64e2cccd62fd8a981c1fed5248b9705fea4/Library/Homebrew/bundle/brew.rb)。
- [rustup 渠道](https://rust-lang.github.io/rustup/concepts/channels.html)、[工具链文件与优先级](https://rust-lang.github.io/rustup/overrides.html#the-toolchain-file)、[固定参考源码](https://github.com/rust-lang/rustup/tree/5c8ff3f907c37688d20d9e7038a2f826042cd763)。
- [uv 工具安装与渠道](https://docs.astral.sh/uv/guides/tools/)、[uv tool install/upgrade 参数](https://docs.astral.sh/uv/reference/cli/#uv-tool-install)。uv upgrade 保留已保存约束；force install 会重建选中环境并替换同名入口。
- [mise Rust 后端](https://mise.jdx.dev/lang/rust.html)、[版本请求](https://mise.jdx.dev/dev-tools/versions.html)、[锁文件](https://mise.jdx.dev/dev-tools/mise-lock.html)。

来源、许可证与实际接入范围见 [ENVIRONMENT_SOURCES.md](ENVIRONMENT_SOURCES.md)。本地回归、目标平台编译和真实机器安装验收分别报告；可生成清单或通过测试并不代表已经安装或完成真实渠道切换。
