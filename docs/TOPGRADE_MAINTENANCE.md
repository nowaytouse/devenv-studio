# 跨平台更新中心

| 平台 | 清单与更新 | 配置和记录 |
| --- | --- | --- |
| Apple Silicon macOS | 查看 Homebrew 已安装清单，审核后执行受控 Topgrade 步骤，其他步骤在终端预演 | 受管 profile 事务、引擎回退和 Studio 更新记录 |
| Windows | 读取 WinGet 匹配的已安装清单，为明确的社区源软件提供单包核对与升级命令 | 进入已有的系统配置、软件部署和维护审核，查看 Studio 部署记录 |

Windows 读取使用 WinGet 的版本化导出，可能访问已配置的源来匹配软件。清单显示导出范围和可获得的版本，升级可用性由终端的核对命令确定。只有官方社区源且 ID 唯一的行提供可复制命令。升级在终端处理，Studio 的部署记录涵盖由 Studio 执行的任务。Windows LTSC 2021 和 2024 仍按各自的目标和主机检查控制配置写入。 [WinGet 导出](https://learn.microsoft.com/en-us/windows/package-manager/winget/export) · [WinGet 核对与升级](https://learn.microsoft.com/en-us/windows/package-manager/winget/upgrade)

## Windows ClamAV 病毒库更新

如果已经在 `%LOCALAPPDATA%\ClamAV\freshclam.conf` 配置了可用的 FreshClam，可以在现有的 `[windows]` 段加入 `clamav_user_config = true`。确保 `freshclam.exe` 在 PATH 中，并启用原生 `clam_av_db` 步骤。迁移时可移除同一病毒库的自定义更新入口，以免重复运行。先用 `topgrade --only clam_av_db --dry-run` 核对配置及调度范围，预演不会更新病毒库。

配置缺失、类型不符，或 ClamAV 子目录和配置文件使用 Windows 重解析点时会报告失败。原生步骤直接调用 FreshClam 更新病毒库，失败时不会尝试提权。病毒库和日志位置由本机 FreshClam 配置决定。 [ClamAV 更新文档](https://docs.clamav.net/manual/Usage/SignatureManagement.html#freshclam)

## macOS Topgrade

当前组件已同步上游 [v17.16.0](https://github.com/topgrade-rs/topgrade/releases/tag/v17.16.0) 与随后提交 `77eef190429850607c41cedfec904f43d66c8394`。Studio 的 macOS 受管步骤和本机 profile 随组件继续维护。滚动依赖维护会跳过有未提交改动的工作区。

`Project Source Indexes` 使用 Repomix 和 LeanCTX 为 `~/Downloads/GitHub` 下的 Git 项目更新私有索引。直接运行 `~/.local/bin/topgrade-project-indexes.py /path/to/checkout` 可单独刷新项目。索引只读取已提交的普通源码文件，先排除私有目录、再执行秘密扫描，最终仅保存相对路径和 token 计数。`${XDG_STATE_HOME:-$HOME/.local/state}/project-indexes/<checkout-name>/latest.json` 记录完整提交、工具版本、UTC 生成时间及检出目录的摘要。刷新时会将过时的已知索引移入 `history/`，将格式、路径或内容边界校验失败的普通私有索引移入 `quarantine/`。原文件保留以备核对。生成失败会报告错误，此时没有已核验的新索引，历史文件仍可查阅。

同目录的 `serena.json` 记录本地 Serena 版本及标准 `.serena/cache` 符号文件的大小、时间和状态。Serena 的符号缓存对应实时检出内容，由 Serena 按文件内容和语言服务器配置判断有效性。索引维护不会反序列化这些缓存，也不会启动语言服务器。结束对应项目的 Serena 会话后，可对明确指定的检出目录运行 `topgrade-project-indexes.py --quarantine-empty-serena-cache /path/to/checkout`，将空的符号缓存移入 `serena-quarantine/`。正常缓存、项目设置和记忆保留。已配置的项目可用 Serena 官方 `serena project index /path/to/checkout` 重建符号缓存，重建时还应核对工具是否报告部分文件失败。 [Serena 项目工作流](https://oraios.github.io/serena/02-usage/040_workflow.html)

「更新中心」从已核验的 Topgrade 引擎读取步骤目录。「更新任务」区分工具与缓存和只读诊断，默认零选中。可审核计数来自本机工具可用性。重跑前需重新检查。「终端维护」置顶 BetterDisplay 与 Homebrew HEAD-sync，并提供其余引擎步骤搜索和本机 profile 预演。预演读取正常 Topgrade 配置，前后置命令及配置中的 `misc.only` 也会参与。以预演结果核对实际范围。配置迁移包仍是独立的备份和提取入口。

「Homebrew 已安装」读取本机 Formula 与 Cask，提供搜索、固定版本筛选和安装、链接、`opt` 详情，并展示读取命令。清单使用 Homebrew 的本地 JSON 列表，读取时关闭自动更新。单包安装、升级和卸载可由 Homebrew 官方应用处理。 [BrewUI](https://github.com/Homebrew/BrewUI)

「更新任务」的审核视图显示每项的命令或运行方式。任务结束后进入「执行记录」，可查看结果和本地输出，也可以独立刷新历史。

在 Apple Silicon 的「更新任务」中，打开「LLVM 手动更新」并一次审核一个渠道。稳定版使用 Homebrew 官方 LLVM 23.1 瓶装，成功回读 LLVM 版本和 Homebrew Rust 后才切换 pin。HEAD 从官方 LLVM main 核对完整提交，构建 `nyamiiko/tools/llvm-head` 并只切换该渠道的 `opt`。审核后的公式、版本或链接发生变化会停止。构建可能耗时数小时，两个入口都保留原 keg；缺少依赖时停止并列出受影响的公式。

首次构建 LLVM HEAD 时，Studio 使用私有 Git 缓存。首次下载只获取官方 main 的提交历史和当前提交的文件，完整提交匹配后才写入缓存。后续构建会复用缓存，Homebrew 仍会在安装前核验实际取到的完整提交。

普通 `topgrade -y` 和 Homebrew HEAD-sync 的范围不含这两个手动入口。LLVM 更新中断时，可从对应 Studio 运行目录的 `llvm-before.json` 核对原 `opt` 和 pin。确认本机状态后，再按下方的中断流程恢复维护。

检查后，「当前更新引擎」显示实际运行版本、组件树短标识及 profile 清单版本和摘要。展开来源可查看二进制路径、Studio 构建提交、完整组件树和两份 SHA-256。组件树区分同一个 Topgrade 版本下的不同源码，二进制摘要进一步区分产物。执行记录保留当时的引擎身份及可用回退候选。旧记录保留已写入的组件树，缺失字段保持未知。

Studio 使用绝对路径启动独立的 Topgrade。审核绑定二进制、受管配置和选中工具的身份。受限入口用 Rust 子进程执行白名单步骤，通过独立请求和事件记录汇报。运行记录和有界日志位于现有 Studio 数据目录的 `maintenance/runs`。普通终端入口、Studio 的相应 Mac 写操作和引擎切换共用维护锁。

当前 Rust 迁移的范围：

| 功能 | 正常 profile | GUI |
| --- | --- | --- |
| GitHub CLI Skills、Helm | 原生验证器保留归属、共享锁和缓存检查 | 可审核执行 |
| Homebrew 链接审计、CLI 版本报告 | Rust 审计与版本探测 | 可审核执行 |
| GitHub CLI 扩展、TLDR | Topgrade 自身步骤 | 可审核执行 |
| gcloud、MAS、Xcode Beta 选择 | Rust 保留重试、核验或回退边界 | 终端入口 |
| Homebrew HEAD-sync | Rust 保留兼容事务日志、固定 LLVM、成对完整 SHA 校验和恢复 | 终端入口 |
| SQLite CLI、Python 3.15 SQLite | Rust 共享源码版本与更新锁，保留事务和运行时验证 | 终端入口 |
| Gradle Nightly | Rust 核对下载和安全解包，验证运行时后切换启动器 | 终端入口 |
| Krita Next、BetterDisplay、TourBox Beta、Xcode Beta 安装 | Rust 核对来源、包身份和安装回读，保留备份或厂商安装机制 | 终端入口 |
| 项目滚动依赖 | Rust 识别项目和依赖面，写入变更前备份与历史，执行受管工具并验证结果 | 终端入口 |
| NPX、设计包和 Cursor 覆盖检查 | Rust 管理归属、镜像、锁和可恢复激活，使用已固定版本的 NPX 更新工具 | 终端入口 |
| npm 捆绑依赖安全 | Rust 验证两份包目录、公告范围及交换事务 | 终端入口 |

旧的 Shell 和 Python 文件随 profile 保留，供兼容入口和回退使用。受管 Topgrade 流程使用 Rust 任务，按需调用 Homebrew、GitHub CLI、npm、Gradle 和厂商安装工具。Python 3.15 SQLite 的 ABI 验证继续通过目标解释器执行。

NPX 清理已确认退役的 `discovering-gcp-data-assets`。核对旧锁记录、本地内容、官方 `main` 中原目录已移除及后继 `discover-assets` 存在后，删除原技能、受管链接及对应锁条目。清理中断后可重试，其他来源与手动副本保留。来源暂时不可用、路径缺失或歧义时，跳过该来源并报告警告，继续检查其他来源。锁完整性、安装和写入失败仍会报错。 [官方添加记录](https://github.com/GoogleCloudPlatform/data-agent-kit-plugin/commit/5422f53e4333fe7f6f382be9689a1369dd8198fe) · [官方退役记录](https://github.com/GoogleCloudPlatform/data-agent-kit-plugin/commit/68924f89e78bc79c04c730eaba1dc8539529865f)

Krita Next 跟随 KDE 官方 `master/macos-universal` 滚动渠道。目录中最新发布的合格构建可以跨版本系列更新，Qt 5、Krita Plus 和稳定版使用独立渠道。同一应用版本的新 Git 提交仍会更新；同 Git SHA 的包若官方发布时间严格晚于收据中的安装时间，也作为重新打包候选核验，缺少或无效时间不推断为更新。候选与复制后的应用必须同时具有一致的 QtCore、QtGui 完整 `CFBundleVersion`，最低为 6.11.3。只有主、次版本号的元数据不能用于准入。没有较新构建证据且已装 Qt 不符合下限时，报告 `BLOCKED_INCOMPATIBLE_QT` 并返回失败；未来的新兼容候选仍可替换旧 Qt 构建。现有发布者、Git 祖先关系、禁止降级、锁、安装收据与交换恢复校验继续生效；成功安装的收据记录 `channel` 和 `qt_version`。兼容 Python 入口使用同一渠道与 Qt 准入规则。

项目滚动更新保留既有依赖面和每仓库策略。npm 检查使用本机已安装的 ncu，禁用依赖安装脚本。Gradle 依赖报告固定使用 0.67.0 插件；wrapper 仅更新校验通过的版本与分发属性，保留原启动脚本和 JAR。共享版本引用影响范围无法证明时，只记录发现的候选。旧独立滚动更新命令和预览应用手动启动选项仍可作为兼容入口。

取消在当前步骤结束后生效。超时或无法证明子进程结束时，保留中断记录，暂停新的维护操作。先检查任务和现存变更，再在终端运行已安装 Topgrade 的 `--studio-acknowledge-interruption`。如果中断记录缺少可核验的进程组信息，先人工确认没有残余进程并核验更新器状态，再保留该记录作恢复依据。

Homebrew 公式步骤先刷新目录，再扫描并升级过期公式。有本地改动的 tap 会保留原样，`homebrew/core` 与其余有上游跟踪分支的干净 tap 仅快进。日志列出保留的 tap 和刷新失败。官方 core 刷新失败时，公式升级停止。升级后再次核对本轮请求的公式。目录延期或仍有公式过期时，摘要显示“部分完成”并返回非零退出码。固定版本和独立管理的公式仍会列出排除原因。

摘要会显示模拟执行和后置命令的结果。显式配置或其引用无法读取或解析时，Topgrade 会在更新前停止。

## 本机构建与切换

源代码来自保留完整历史的 Topgrade 组件，使用独立 Cargo 配置及原许可证。维护者从干净、审核过的 Studio checkout 操作：

```sh
devenv-studio topgrade-engine plan /absolute/studio-checkout FULL_STUDIO_SHA
devenv-studio topgrade-engine prepare /absolute/studio-checkout FULL_STUDIO_SHA
devenv-studio topgrade-engine install /absolute/studio-checkout FULL_STUDIO_SHA
```

`prepare` 构建并核验固定的组件树，保存候选引擎及摘要。 `install` 使用 Studio 的 Rust 安装器验证资产、应用并回读 profile，然后原子更新 `~/.local/bin/topgrade` 指向的项目引擎。安装器兼容原有 v1 收据和事务日志。相同组件树复用引擎。软件更新使用已部署的项目 fork。旧的 `topgrade-rustup-headsync` 仅提示 Studio 的维护入口，所有模式都停止执行。

### 跟进 Topgrade 官方版本

`topgrade-engine check-upstream /absolute/studio-checkout` 从该 checkout 当前提交读取 `topgrade-upstream.json`，通过 HTTPS 查询 Topgrade 官方 Git 标签，按版本号比较稳定版。输出记录 Studio 提交、已审核的标签对象和提交、当前最高稳定标签及检查时间。新版本显示 `new_release`。已审核标签被移动或消失时显示 `pin_changed` 或 `pin_missing`。普通版本检查只读 Git 引用，无需 GitHub 凭据。

发现新版本后，在隔离工作树导入并审核组件源码、Cargo 锁文件及本机 profile，然后在同一 Studio 提交更新上游 pin。运行 `devenv-studio topgrade-engine prepare-upstream /absolute/studio-checkout FULL_STUDIO_SHA` 重新核对官方标签，再从该干净提交构建候选。标签对象、完整提交和组件版本均须一致。候选构建成功后仍须显式执行上面的 `install`，原有回退流程可继续使用。

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

JPEG XL 待升级时，HEAD-sync 先单独处理这组配方。它从 Homebrew 已安装的依赖图出发，检查 keg 的实际动态库引用来识别需要重建的依赖方，保留原 tap、HEAD 或稳定渠道及构建选项。开始写入前保存可核验的旧 keg 和事务记录。构建或验收失败时恢复原安装，异常中断后的下一次 HEAD-sync 先恢复该事务。只有新链接和运行验证通过后才移出旧 JPEG XL keg，再重新检查全机链接，以捕获依赖图以外的损坏。其余配方随后重新枚举并升级。

Mac App Store 已报告安装成功后，若精确核验暂时找不到本轮刚更新的应用 ID，最多等待 15 秒并复查。网络、授权、其他应用 ID 或无法验证的结果继续返回失败。

检查显示上一版时，可复制绑定两版组件树的终端回退命令。请关闭其他维护任务后运行。回退重新核对当前与上一版引擎、profile 和事务，若身份或配置发生漂移则停止。回退完成后重新检查更新中心，或用 `devenv-studio topgrade-engine verify-current FULL_COMPONENT_TREE` 核对启动器及全部受管资产。相同 checkout 和 SHA 的 `topgrade-engine rollback` 仍可用于维护入口。若过程报告 pending 激活，请保留文件、启动器和 profile 事务，核对实际生效版本后再恢复。私人记录在 `~/.local/share/topgrade-rustup-head` 和 `~/.local/state/topgrade-macos-profile`。

## 设备验收

macOS 先独立核对每个写入任务的目标、版本或缓存摘要，再运行已审核的单项，保存退出状态和读回结果。用相同范围复跑，记录成功变更、已是最新、跳过或失败。完整终端运行前另行核对有效 profile 的前置、后置和自定义命令，以及需要授权的任务。完整运行通过后重复同一范围，核对第二轮变化。

TLDR 刷新会按 tealdeer 选择的下载语言替换缓存。多语言用户应在 `tldr --show-paths` 显示的配置文件中显式指定 `[updates]` 下的 `download_languages`，例如 `["en", "zh"]`。受管 GUI 进程使用受限环境，单靠交互 shell 的语言变量可能让刷新丢失中文等缓存。验收时分别核对各语言页面。 [tealdeer 下载配置](https://docs.tealdeer.org/latest/config_updates.html)

Windows 在 LTSC 2021 和 LTSC 2024 分开记录系统版本、预览产物摘要、Studio 启动、WinGet 清单和实际权限提示。对同一来源中有唯一 ID 的非关键包，先记录已安装版本和明确目标版本，再执行 Studio 显示的单包终端命令，核对退出码和升级后的版本。两代设备各自通过后再开放 GUI 单包写入。

### Windows DNS 隔离验收

在 FlClash 的本地文件中保留 `dns.nameserver-policy`，导入后从生效配置核对该策略、HTTPS 上游、TUN 严格路由及 UDP／TCP 端口 53 接管。普通文件配置只接受部分全局覆盖字段，因此必须核对生效配置。记录 IPv6 与 LLMNR 的独立状态。维护记录中通过的组合使用原有的核心 IPv6 设置，LLMNR 已关闭，智能多宿主名称解析策略未配置。 [FlClash 配置合成源码](https://github.com/chen08209/FlClash/blob/v0.9.3/lib/providers/state/profile.dart) · [Mihomo TUN 配置](https://wiki.metacubex.one/en/config/inbound/tun/)

使用合成域名分别发出 UDP／TCP DNS 查询，再用 HTTPS 请求作正对照。只在短暂测试窗口对物理网卡统计端口 53 和 443 的出站事件及捕获丢失量。报告仅保存计数和测试范围，抓包内容留在本机并在测试结束后清理。分别记录 TUN 运行中、退出后和重启后的结果。发现连通性回退时恢复上一份已验收的客户端配置。

Studio 预览包包含 Studio 自己的可执行文件。引擎由本机单独构建。仓库内保留 Topgrade 源代码、第三方声明及 GPL 许可；分发引擎时须一并处理对应源码与构建材料。
