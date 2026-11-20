# 跨平台更新中心

| 平台 | 清单与更新 | 配置和记录 |
| --- | --- | --- |
| Apple Silicon macOS | 查看 Homebrew 已安装清单，审核后执行受控 Topgrade 步骤，其他步骤在终端预演 | 受管 profile 事务、引擎回退和 Studio 更新记录 |
| Windows | 读取 WinGet 匹配的已安装清单，为明确的社区源软件提供单包核对与升级命令 | 进入已有的系统配置、软件部署和维护审核，查看 Studio 部署记录 |

Windows 读取使用 WinGet 的版本化导出，可能访问已配置的源来匹配软件。清单显示导出范围和可获得的版本，升级可用性由终端的核对命令确定。只有官方社区源且 ID 唯一的行提供可复制命令。升级在终端处理，Studio 的部署记录涵盖由 Studio 执行的任务。Windows LTSC 2021 和 2024 仍按各自的目标和主机检查控制配置写入。 [WinGet 导出](https://learn.microsoft.com/en-us/windows/package-manager/winget/export) · [WinGet 核对与升级](https://learn.microsoft.com/en-us/windows/package-manager/winget/upgrade)

## Windows 更新完成状态

Scoop 使用现有 PowerShell 入口并显式传播子脚本退出码。更新后读取本地状态对象，仍有非 held 的过期包、失败安装、缺失依赖或已移除清单时报告未完成。状态按结构化字段判断。只有确认更新完成才执行可选清理。预演不会运行更新或额外状态查询。

WinGet 在升级后导出来源清单，并逐个核对默认参与搜索的来源，逐一检查所有适用来源，来源错误优先于剩余候选导致的部分完成。显式来源、用户的版本限制及未知版本包继续遵守 WinGet 原有范围；剩余候选报告未完成。该核验不等于覆盖未纳入 WinGet 的软件。

WSL 原生更新遇到不随界面语言变化的 HTTP 403／429 错误码时，可通过 WinGet 社区源的精确 `Microsoft.WSL` 稳定包更新或确认无适用更新。其他安装错误继续失败，预发布配置不会改走稳定渠道；备用升级返回成功后还会核对该包的剩余候选。GitHub 的匿名请求限额可能自行恢复，不把一次重试成功当作已永久消除限流。

GitHub CLI 技能更新先读取原生 JSON 跟踪状态；未 pin 且缺少来源的副本及原生警告会报告未完成，即使原命令最后输出已是最新并返回 0。没有符合条件的 GitHub 跟踪项时，直接返回盘点结果。其他安装器管理的副本保留原有文件和元数据。自定义更新器与 macOS 原生验证器保持各自的责任范围。

## Windows ClamAV 病毒库更新

如果已经在 `%LOCALAPPDATA%\ClamAV\freshclam.conf` 配置了可用的 FreshClam，可以在现有的 `[windows]` 段加入 `clamav_user_config = true`。确保 `freshclam.exe` 在 PATH 中，并启用原生 `clam_av_db` 步骤。迁移时可移除同一病毒库的自定义更新入口，以免重复运行。先用 `topgrade --only clam_av_db --dry-run` 核对配置及调度范围，预演不会更新病毒库。

配置缺失、类型不符，或 ClamAV 子目录和配置文件使用 Windows 重解析点时会报告失败。原生步骤直接调用 FreshClam 更新病毒库，失败时不会尝试提权。病毒库和日志位置由本机 FreshClam 配置决定。 [ClamAV 更新文档](https://docs.clamav.net/manual/Usage/SignatureManagement.html#freshclam)

## macOS Topgrade

当前组件已同步上游 [v17.16.0](https://github.com/topgrade-rs/topgrade/releases/tag/v17.16.0) 与随后官方 main 提交 [`b7ae844e64fb3bd32cd06c00859884f8b916635d`](https://github.com/topgrade-rs/topgrade/commit/b7ae844e64fb3bd32cd06c00859884f8b916635d)。在原同步点 `77eef190429850607c41cedfec904f43d66c8394` 之后，继续纳入 Vim／Neovim 的可选 `vim.show_output` 配置与官方发布证明工作流更新；程序版本仍为 17.16.0，官方发布跟踪文件继续记录已审核的稳定版本标签。Studio 的 macOS 受管步骤和本机 profile 随组件继续维护。滚动依赖维护会跳过有未提交改动的工作区。

`Project Source Indexes` 使用 Repomix 和 LeanCTX 为 `~/Downloads/GitHub` 下的 Git 项目更新私有索引。直接运行 `~/.local/bin/topgrade-project-indexes.py /path/to/checkout` 可单独刷新项目。索引只读取已提交的普通源码文件，先排除私有目录、再执行秘密扫描，最终仅保存相对路径和 token 计数。`${XDG_STATE_HOME:-$HOME/.local/state}/project-indexes/<checkout-name>/latest.json` 记录完整提交、工具版本、UTC 生成时间及检出目录的摘要。刷新时会将过时的已知索引移入 `history/`，将格式、路径或内容边界校验失败的普通私有索引移入 `quarantine/`。原文件保留以备核对。生成失败会报告错误，此时没有已核验的新索引，历史文件仍可查阅。

同目录的 `serena.json` 记录本地 Serena 版本及标准 `.serena/cache` 符号文件的大小、时间和状态。Serena 的符号缓存对应实时检出内容，由 Serena 按文件内容和语言服务器配置判断有效性。索引维护不会反序列化这些缓存，也不会启动语言服务器。结束对应项目的 Serena 会话后，可对明确指定的检出目录运行 `topgrade-project-indexes.py --quarantine-empty-serena-cache /path/to/checkout`，将空的符号缓存移入 `serena-quarantine/`。正常缓存、项目设置和记忆保留。已配置的项目可用 Serena 官方 `serena project index /path/to/checkout` 重建符号缓存，重建时还应核对工具是否报告部分文件失败。 [Serena 项目工作流](https://oraios.github.io/serena/02-usage/040_workflow.html)

「更新中心」从已核验的 Topgrade 引擎读取步骤目录。「更新任务」区分工具与缓存和只读诊断，默认零选中。可审核计数来自本机工具可用性。重跑前需重新检查。「终端维护」置顶 BetterDisplay 与 Homebrew HEAD-sync，并提供其余引擎步骤搜索和本机 profile 预演。预演读取本机 profile。命令行显式指定 `--only` 时，以该步骤列表覆盖 `misc.only`，并跳过全局前后置命令。配置迁移包仍是独立的备份和提取入口。

「Homebrew 已安装」读取本机 Formula 与 Cask，提供搜索、固定版本筛选和安装、链接、`opt` 详情，并展示读取命令。清单使用 Homebrew 的本地 JSON 列表，读取时关闭自动更新。单包安装、升级和卸载可由 Homebrew 官方应用处理。 [BrewUI](https://github.com/Homebrew/BrewUI)

「更新任务」的审核视图显示每项的命令或运行方式。任务结束后进入「执行记录」，可查看结果和本地输出，也可以独立刷新历史。

在 Apple Silicon 的「LLVM 工具链」中，用滚动 LLVM 跟进官方 LLVM main。Studio 核对完整提交，构建 `nyamiiko/tools/llvm-head`，并验证该工具链的 `opt` 和版本。需要命令行使用时，从 `brew --prefix llvm-head` 对应的 `bin` 选择工具。Xcode 仍使用自己的 SDK 和工具链。

Homebrew 的 `llvm` 仅用于兼容已安装配方。Homebrew Rust 可能在动态链接表中写入该配方的绝对路径和精确 LLVM ABI。应先迁移依赖，再让 Homebrew 自己验证并移除旧提供者。兼容依赖仍存在时，只接受已安装 ABI 系列的官方瓶装，验证 LLVM 和 Homebrew Rust 后才切换 pin。构建和审核中途发现配方、版本或链接变化时停止并保留原 keg。

首次构建 LLVM HEAD 时，Studio 使用私有 Git 缓存。首次下载只获取官方 main 的提交历史和当前提交的文件，完整提交匹配后才写入缓存。后续构建会复用缓存，Homebrew 仍会在安装前核验实际取到的完整提交。

普通 `topgrade -y` 默认跟进已安装的滚动 LLVM。有现存 Homebrew 兼容依赖时，HEAD-sync 在升级其余配方前核验并在同 ABI 系列内推进兼容瓶装。兼容 keg、`opt` 和 pin 均已清理且 Homebrew 报告依赖完整时，就只处理滚动 LLVM。配方预检如果要求隐式安装或升级 LLVM，会在写入前停止。更新中断时保留对应事务记录，供检查原 `opt` 和 pin。

LLVM 预览版的工作进程失败且事务恢复已确认时，本轮显示“部分完成”，已完成的 Homebrew 阶段不会被自动重试。恢复未完成时保留日志并报告失败。

媒体配方的状态来自已有的 Homebrew `--fetch-HEAD` 扫描和升级后复核。日志会显示哪些配方已检查、已有更新、已更新、仍然过期或因 pin 跳过。该状态按 Homebrew 的候选结果判定，完整 Git SHA 需要从上游和安装收据另行核对。

技能目录按来源管理。GitHub 和 NPX 技能沿用客户端的发现路径及原有元数据，Cursor 的预置技能由 `~/.cursor/skills-cursor` 提供，经核验后同步到 `~/.agents/skills`。整份 Cursor 镜像缺失时从同名原生源恢复，源不可用或现有目录无法核验时保留现状并报告原因。手工改写的共享技能可以在 `~/.local/share/devenv-studio/reviewed-skills/<技能名>` 保存一份私有审核副本，目录权限设为 `0700`。审核时逐项比较两份目录的文件路径和内容，确认一致后显示“已审核的本地技能”，留在原路径供客户端使用。更新器不会改写这类技能或审核副本，修改后需重新核对审核副本。覆盖报告按 GitHub、NPX、Cursor 和审核本地副本列出归属。

检查后，「当前更新引擎」显示实际运行版本、组件树短标识及 profile 清单版本和摘要。展开来源可查看二进制路径、Studio 构建提交、完整组件树和两份 SHA-256。组件树区分同一个 Topgrade 版本下的不同源码，二进制摘要进一步区分产物。执行记录保留当时的引擎身份及可用回退候选。旧记录保留已写入的组件树，缺失字段保持未知。

Studio 使用绝对路径启动独立的 Topgrade。审核绑定二进制、受管配置和选中工具的身份。受限入口用 Rust 子进程执行白名单步骤，通过独立请求和事件记录汇报。运行记录和有界日志位于现有 Studio 数据目录的 `maintenance/runs`。普通终端入口、Studio 的相应 Mac 写操作和引擎切换共用维护锁。

当前 Rust 迁移的范围：

| 功能 | 正常 profile | GUI |
| --- | --- | --- |
| GitHub CLI Skills、Helm | 原生验证器保留归属、共享锁和缓存检查 | 可审核执行 |
| Homebrew 链接审计、CLI 版本报告 | Rust 审计与版本探测 | 可审核执行 |
| GitHub CLI 扩展、TLDR | Topgrade 自身步骤 | 可审核执行 |
| gcloud、MAS、Xcode Beta 选择 | Rust 保留重试、核验或回退边界 | 终端入口 |
| Homebrew HEAD-sync | Rust 跟进滚动 LLVM，保护现存的 Homebrew ABI 兼容依赖，并核验配对完整 SHA 和事务恢复 | 终端入口 |
| SQLite CLI、Python 3.15 SQLite | Rust 共享源码版本与更新锁，保留事务和运行时验证 | 终端入口 |
| Gradle Nightly | Rust 核对下载和安全解包，验证运行时后切换启动器 | 终端入口 |
| Krita Next、BetterDisplay、TourBox Beta、Xcode Beta 安装 | Rust 核对来源、包身份和安装回读，保留备份或厂商安装机制 | 终端入口 |
| 项目滚动依赖 | Rust 识别项目和依赖面，写入变更前备份与历史，执行受管工具并验证结果 | 终端入口 |
| NPX、设计包和 Cursor 覆盖检查 | Rust 管理归属、镜像、锁和可恢复激活，使用已固定版本的 NPX 更新工具 | 终端入口 |
| npm 捆绑依赖安全 | Rust 验证两份包目录、公告范围及交换事务 | 终端入口 |

旧的 Shell 和 Python 文件随 profile 保留，供兼容入口和回退使用。受管 Topgrade 流程使用 Rust 任务，按需调用 Homebrew、GitHub CLI、npm、Gradle 和厂商安装工具。Python 3.15 SQLite 的 ABI 验证继续通过目标解释器执行。

NPX 清理已确认退役的 `discovering-gcp-data-assets`。核对旧锁记录、本地内容、官方 `main` 中原目录已移除及后继 `discover-assets` 存在后，删除原技能、受管链接及对应锁条目。清理中断后可重试，其他来源与手动副本保留。来源暂时不可用、路径缺失或歧义时，跳过该来源并报告警告，继续检查其他来源。锁完整性、安装和写入失败仍会报错。 [官方添加记录](https://github.com/GoogleCloudPlatform/data-agent-kit-plugin/commit/5422f53e4333fe7f6f382be9689a1369dd8198fe) · [官方退役记录](https://github.com/GoogleCloudPlatform/data-agent-kit-plugin/commit/68924f89e78bc79c04c730eaba1dc8539529865f)

Krita Next 跟随 KDE 官方 `master/macos-universal` 滚动渠道。目录中最新发布的合格构建可以跨版本系列更新，Qt 5、Krita Plus 和稳定版使用独立渠道。同一应用版本的新 Git 提交仍会更新；同 Git SHA 的包若官方发布时间严格晚于收据中的安装时间，也作为重新打包候选核验，缺少或无效时间不推断为更新。候选与复制后的应用必须同时具有一致的 QtCore、QtGui 完整 `CFBundleVersion`，最低为 6.11.3。只有主、次版本号的元数据不能用于准入。没有较新构建证据且已装 Qt 不符合下限时，报告 `BLOCKED_INCOMPATIBLE_QT` 并返回失败；未来的新兼容候选仍可替换旧 Qt 构建。现有发布者、Git 祖先关系、禁止降级、锁、安装收据与交换恢复校验继续生效；成功安装的收据记录 `channel` 和 `qt_version`。兼容 Python 入口使用同一渠道与 Qt 准入规则。

GitHub CLI 对 Gemini 中已核对的旧版 `discovering-gcp-data-assets` 单独迁移。先验证官方 `discover-assets` 候选，再切换并归档旧文件。有待更新的 GitHub 技能时，先用独立临时目录复现已安装版本，再核验并原子替换候选。替换后保留上一份目录。共享 NPX 锁逐字节保留。存在本地修改或无法复现旧版的技能报告延期。

项目滚动更新从 Git 跟踪的依赖清单识别更新范围。与受版本控制的 `pyproject.toml` 同目录的 `uv.lock` 也可随项目更新。被忽略的本地工具和参考项目不会自行进入更新计划。Cargo 自动更新尊重清单中的精确版本固定。带有未提交改动的仓库会在执行更新前延期，摘要按仓库列出待处理项。完成的自动更新记录原始提交、更新前后摘要和文件模式。后续运行只有在这些证据与当前草稿完全相符时才报告待审核的自动更新，并指向相应记录。BetterDisplay 使用官方 Sparkle 源和指定渠道进行更新检查，应用自身管理内部频道偏好。npm 检查使用本机已安装的 ncu，禁用依赖安装脚本。Gradle 依赖报告固定使用 0.67.0 插件；wrapper 仅更新校验通过的版本与分发属性，保留原启动脚本和 JAR。共享版本引用影响范围无法证明时，只记录发现的候选。旧独立滚动更新命令和预览应用手动启动选项仍可作为兼容入口。

取消在当前步骤结束后生效。超时或无法证明子进程结束时，保留中断记录，暂停新的维护操作。先检查任务和现存变更，再在终端运行已安装 Topgrade 的 `--studio-acknowledge-interruption`。如果中断记录缺少可核验的进程组信息，先人工确认没有残余进程并核验更新器状态，再保留该记录作恢复依据。

查询 Docker Hub 的官方 `library` 镜像标签与摘要时，使用临时空的凭据配置，避免调用系统凭据助手。其他镜像仍使用现有认证配置。限流和网络失败继续报告为未完成。

Homebrew 公式步骤先刷新目录，再扫描并升级过期公式。所有 tap 干净时使用常规 `brew update`。存在本地改动时，公式步骤改用 Git 刷新 `homebrew/core` 和 tap 目录，并保留本地补丁。包含最新上游提交的 tap 直接报告当前，缺少的提交只在能快进且来件路径与本地改动不重叠时合入。分叉、路径重叠或缺少跟踪分支会延期，日志列出原因。这个分支的目录刷新使用 Git 中的公式，Homebrew 程序和 API 缓存由常规 `brew update` 刷新。仅指向自身的未跟踪 tap 显示为本地专用目录。官方 core 刷新失败时，公式升级停止。升级后再次核对本轮请求的公式。目录延期或仍有公式过期时，摘要显示“部分完成”并返回非零退出码。固定版本和独立管理的公式仍会列出排除原因。

tap 负责构建配方和自定义参数。已经按 HEAD 安装的 `nyamiiko/tools/libavif` 由公式中的官方 Git `main` 和 Topgrade 的 `--fetch-HEAD` 跟踪。只有上游改动触及构建要求、依赖版本、自定义参数或验证时，才需要更新这份公式。

摘要会显示模拟执行和后置命令的结果。显式配置或其引用无法读取或解析时，Topgrade 会在更新前停止。

## 本机构建与切换

Studio 桌面端与 Topgrade 引擎共用一个 Cargo 工作区、锁文件和 CI。GUI 继续通过现有命令和运行记录调用引擎，Topgrade 保持独立执行入口与原许可证。旧独立 fork 已归档，上游变更统一在这个仓库审核。

从旧结构首次升级时，先在本仓库构建新版 `devenv-studio`，再使用该版本运行 `topgrade-engine update`。旧版安装器只会导出旧的独立引擎目录，无法读取共用锁文件。

更新器读取 Studio checkout 已配置的跟踪分支。工作区干净且能快进时，使用 Rust nightly 校验并构建实际跟踪的 HEAD。通过验证的候选文件与默认入口位于同一文件系统，最后一次重命名会将候选启用为 `~/.local/bin/topgrade` 普通文件。构建产物和验证目录只用于本次更新，引擎身份和摘要保存在当前文件的扩展属性中。

```sh
devenv-studio topgrade-engine plan /absolute/studio-checkout
devenv-studio topgrade-engine update /absolute/studio-checkout
devenv-studio topgrade-engine verify-current /absolute/studio-checkout
```

已部署的 `~/.local/bin/topgrade-rustup-headsync update /absolute/studio-checkout` 调用同一个更新入口。它需要 `~/.local/bin/devenv-studio` 可执行。profile 仍按已记录的前镜像和原子文件替换流程部署。更新器识别现有的默认符号链接和普通文件，保留配置与现有事务记录。

### 跟进 Topgrade 官方版本

`topgrade-engine check-upstream /absolute/studio-checkout` 从该 checkout 当前提交读取 `topgrade-upstream.json`，通过 HTTPS 查询 Topgrade 官方 Git 标签，按版本号比较稳定版。输出记录 Studio 提交、已审核的标签对象和提交、当前最高稳定标签及检查时间。新版本显示 `new_release`。已审核标签被移动或消失时显示 `pin_changed` 或 `pin_missing`。普通版本检查只读 Git 引用，无需 GitHub 凭据。

发现官方新版本后，在隔离工作树导入并审核组件源码、共用 Cargo 锁文件及本机 profile，然后在同一 Studio 提交更新上游 pin。合入 Studio 跟踪分支后，常规滚动更新器会从新的项目 HEAD 构建。

需要每日自动检查时，可自行启用只读 LaunchAgent。请从你信任且长期保留的 Studio 二进制生成文件，将示例中的两条绝对路径换成本机实际位置：

```sh
agent="$HOME/Library/LaunchAgents/org.devenv-studio.topgrade-upstream.plist"
mkdir -p "$HOME/Library/LaunchAgents"
(umask 077; set -C; /absolute/bin/devenv-studio topgrade-engine export-upstream-agent /absolute/studio-checkout > "$agent")
/usr/bin/plutil -lint "$agent"
/bin/launchctl bootstrap "gui/$(id -u)" "$agent"
```

生成的配置只安排 `record-upstream`。首次加载和此后每日查询时，它将最新成功检查原子写入 `~/Library/Application Support/DevEnv Studio Topgrade Upstream/status-v1.json`。用 `launchctl print "gui/$(id -u)/org.devenv-studio.topgrade-upstream"` 查看任务状态和最近退出码，并核对 JSON 检查时间。断网或检查失败会保留上次记录。通过 `launchctl bootout "gui/$(id -u)/org.devenv-studio.topgrade-upstream"` 卸载；如需跨登录停用，将对应 plist 移出 `~/Library/LaunchAgents`。

原有 Homebrew HEAD-sync 日志仍可由 Rust 入口恢复。组件在执行两项成对升级后先验证两份活动 keg 的完整 SHA，再枚举其余公式。收到 TERM 或 HUP 时，更新器持锁等待正在运行的子进程，随后恢复两份公式并以相应的退出状态结束。若第一项 keg 升级成功而第二项失败，请保留新旧 keg 和日志，核对两个 `opt` 指向与完整收据后再审阅配对恢复。若只有父进程被强制杀死，请先确认原来的 Homebrew 和构建子进程均已退出，再运行恢复。

HEAD-sync 在刷新目录前检查所有 tap。未解决的 Git 冲突会在构建前停止。有本地改动且无冲突时，暂缓 `brew update`，使用现有目录和配方继续核验。干净的 tap 完成刷新后会再次检查。启用受管 HEAD-sync 时，Apple Silicon 的 Cask 步骤关闭隐式目录刷新。两份活动 keg 已匹配目标完整 SHA 时，直接核验现有运行时，避免因后续公式失败而重建同一配对。普通批量升级保留 LLVM、LLVM HEAD 和版本化 LLVM 配方。

ggml 和 llama.cpp 仍同步到同一个完整 HEAD 提交。匹配后检查已安装依赖方的实际动态库路径，只重建仍引用旧 ABI 的包。重建前备份受影响的 keg，失败时恢复。对官方稳定版 Whisper 1.9.5 和 llama 1.x 的已知 CMake 兼容问题，暂时调整公式中的版本匹配，保留上游源码和原有命令集，随后恢复官方公式。没有待升级公式时也执行最终链接审计，坏链接会使步骤失败。

JPEG XL 待升级时，HEAD-sync 先单独处理这组配方。它从 Homebrew 已安装的依赖图出发，检查 keg 的实际动态库引用来识别需要重建的依赖方，保留原 tap、HEAD 或稳定渠道及构建选项。开始写入前保存可核验的旧 keg，并将重建所需的依赖升级目标一并记入事务日志。Homebrew 每次实际写入依赖前都要确认目标符合计划，并持久记录该次写入。构建或验收失败时恢复原安装，异常中断后的下一次 HEAD-sync 先恢复该事务。只有新链接和运行验证通过后才移出旧 JPEG XL keg，再重新检查全机链接，以捕获依赖图以外的损坏。其余配方随后重新枚举并升级。即使 JPEG XL 本身已是当前版本，HEAD-sync 也会检查已安装依赖方。若瓶装配方仍引用旧 ABI，只重建受影响的依赖方，保留当前 JPEG XL；构建或核验失败时按清单恢复。

Mac App Store 已报告安装成功后，若精确核验暂时找不到本轮刚更新的应用 ID，最多等待 15 秒并复查。网络、授权、其他应用 ID 或无法验证的结果继续返回失败。

更新中心读取当前引擎身份和配置状态。终端可通过 `topgrade-engine verify-current /absolute/studio-checkout` 核对当前引擎及受管文件。若存在未完成的旧激活事务，先核对其记录与本机状态。历史日志和 profile 事务仍保留在原路径。

## 设备验收

macOS 先独立核对每个写入任务的目标、版本或缓存摘要，再运行已审核的单项，保存退出状态和读回结果。用相同范围复跑，记录成功变更、已是最新、跳过或失败。完整终端运行前另行核对有效 profile 的前置、后置和自定义命令，以及需要授权的任务。完整运行通过后重复同一范围，核对第二轮变化。

TLDR 刷新会按 tealdeer 选择的下载语言替换缓存。多语言用户应在 `tldr --show-paths` 显示的配置文件中显式指定 `[updates]` 下的 `download_languages`，例如 `["en", "zh"]`。受管 GUI 进程使用受限环境，单靠交互 shell 的语言变量可能让刷新丢失中文等缓存。验收时分别核对各语言页面。 [tealdeer 下载配置](https://docs.tealdeer.org/latest/config_updates.html)

Windows 在 LTSC 2021 和 LTSC 2024 分开记录系统版本、预览产物摘要、Studio 启动、WinGet 清单和实际权限提示。对同一来源中有唯一 ID 的非关键包，先记录已安装版本和明确目标版本，再执行 Studio 显示的单包终端命令，核对退出码和升级后的版本。两代设备各自通过后再开放 GUI 单包写入。

### Windows DNS 隔离验收

在 FlClash 的本地文件中保留 `dns.nameserver-policy`，导入后从生效配置核对该策略、HTTPS 上游、TUN 严格路由及 UDP／TCP 端口 53 接管。普通文件配置只接受部分全局覆盖字段，因此必须核对生效配置。记录 IPv6 与 LLMNR 的独立状态。维护记录中通过的组合使用原有的核心 IPv6 设置，LLMNR 已关闭，智能多宿主名称解析策略未配置。 [FlClash 配置合成源码](https://github.com/chen08209/FlClash/blob/v0.9.3/lib/providers/state/profile.dart) · [Mihomo TUN 配置](https://wiki.metacubex.one/en/config/inbound/tun/)

使用合成域名分别发出 UDP／TCP DNS 查询，再用 HTTPS 请求作正对照。只在短暂测试窗口对物理网卡统计端口 53 和 443 的出站事件及捕获丢失量。报告仅保存计数和测试范围，抓包内容留在本机并在测试结束后清理。分别记录 TUN 运行中、退出后和重启后的结果。发现连通性回退时恢复上一份已验收的客户端配置。

Studio 预览包包含 Studio 自己的可执行文件。引擎由本机单独构建。仓库内保留 Topgrade 源代码、第三方声明及 GPL 许可；分发引擎时须一并处理对应源码与构建材料。
