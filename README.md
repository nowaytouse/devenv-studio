# DevEnv Studio

用 Rust 构建的 macOS 与 Windows 开发环境备份、配置工作台。两端均可采集本机开发工具并选择 mise 核心运行时重部署；macOS 另有 Homebrew Bundle、NPM 全局包与 VS Code 扩展重部署，Windows 提供 WinGet、Cargo、uv、Pip、NPM 和 VS Code 的可信任务。Windows LTSC 系统设置继续作为独立区域，Linux 可载入、预览和导出。

当前开发基线为 **0.10.0**；更名及后续改进记录在 **Unreleased**，不会随每轮对话递增版本。正式版本只在经过审查的发布检查点确定，规则见[版本与发布](docs/RELEASING.md)。

将多个上游的配置和软件选择整合为同一套 Rust 清单、配置档案、计划与执行流程。系统设置提供 58 个选项，覆盖 40 组独立设置；另有 234 个 WinGet 软件包、63 个开发工具/Python包/扩展、7 个 Windows 可选功能、4 个系统维护任务、4 个存储分析/优化任务，以及五组16个插电电源选项。所有选择默认关闭。

## 旧项目整合与退役

DevEnv Studio 现在是唯一维护入口。`LTSC_Tools_Backup` 的配置、映射、部署器、平台适配、原值记录和有界命令执行已进入同一 Rust 工作区，GUI 的**扩展部署**直接通过本应用的任务、取消和共享变更锁运行这些能力，不再启动独立旧程序。

扩展部署保留旧包矩阵、Scoop、Git/PowerShell、VS Code/Cursor 设置与扩展、Ollama、Windows 网络/TCP、TRIM/存储及电源/系统配置。所有领域与软件项目初始关闭；按提供程序筛选、搜索和逐项选择后，实际审核及执行只接收所选项目。Store 应用也必须明确选择，不再随附加软件隐含安装。完整 Profile 仍可高级编辑，编辑后清空项目选择与确认。macOS 可审阅，真实执行仍限受支持 Windows。

扩展部署请求可保存为本地 JSON，再载入继续审核；文件记录候选 Profile、领域、目标和项目选择，不保存执行确认，也不自动安装。载入失败保留当前草稿，保存不覆盖已有文件；来源是当前请求和内置映射，无需访问已退役旧仓库。此页是高级兼容工作流，不把自定义包或旧原值记录称为现代受信目录及认证恢复。

未审查的 Agent Skills/Plugins、字体与个人清单不装入新应用；旧未认证 `RollbackLatest` 不作为可执行入口。旧 main 的全部历史已并入本项目，旧 GitHub 仓库维持只读归档；经所有者授权，旧本地目录在完整私有归档和逐文件核验后删除。原始 EXE、未提交依赖改动和本地检查点保存在仓库外，不再保留第二个开发入口；详细来源、实际范围与退役记录见[整合记录](docs/LEGACY_MERGE.md)。

## 名称与兼容性

应用默认进入环境配置，导航分为跨平台环境、Windows 配置、执行记录与应用设置。

项目现名 **DevEnv Studio**，仓库为 [nowaytouse/devenv-studio](https://github.com/nowaytouse/devenv-studio)，主命令为 `devenv-studio`。旧命令 `ltsc-studio` 继续调用同一实现；源码工作目录不需要移动。

偏好、网络方案与恢复历史继续读取原有 `LTSC Studio` 应用数据目录，bundle identity `io.github.nowaytouse.ltsc-studio`、档案格式和认证标识保持稳定。它们是兼容性契约，保留旧名称不影响界面使用新品牌。

## 双平台开发环境

首次使用从环境配置页的三步入口开始：保存环境备份 → 一键准备工具 → 核对后配置。Mac 基础方案只预选工具、使用补齐模式，并定位到工具审核区域；安装仍需已保存且重新核验的 Mac 清单和明确确认。Windows 继续按用途选择现有方案，再进入变更方案审核。右上角“? 使用帮助”提供当前页面的三步说明和返回入口，帮助往返保留搜索与选择；完整手册可展开查看。

环境备份页在 macOS 采集 Homebrew formula/cask、Cargo、uv、NPM 全局包、VS Code 扩展；Windows 采集 Cargo、uv、NPM、一个已注册标准 CPython 可见的 Pip 分发和 VS Code 扩展。mise core 已安装运行时在环境配置页单独只读采集；可选择显示版本并保存为严格请求，作为后续重部署备份。采集使用本地有界命令，不安装、更新或引导工具。每类来源显示完整、未找到或失败；失败和取消保留上次结果，未找到不等于软件已卸载。Windows 软件全集、所有 Python 环境和项目依赖不在备份范围内。

环境配置页提供 macOS 基础工具一键预选和 Windows 用途方案入口；它们只准备待审选择，不会跳过变更计划的核对与确认。采集后可搜索、保存新 JSON 备份、重新载入，再进入环境配置：Mac 选择基础工具或备份中的 Homebrew 项目，审核生成的 Brewfile，导出或明确在本机配置；mise 可在 macOS/Windows 安装选中的核心运行时。Windows 从备份审核可信任务，也可选择现有混合组合和软件目录；迁移初始不勾选。mise 安装不会修改 Shell 激活或项目/全局配置，完成后导出的固定版 mise.toml 与锁文件需一并审核和放入目标项目。只有完整采集的来源参与映射；包版本保留为观测信息，不被偷换成 Windows 版本锁定。备份只包含工具清单和版本，不复制二进制、项目、密钥、配置文件或虚拟环境，也不是一键精确还原。文件保留在本地，不上传 GitHub。Mac 执行前需保存并审核 Homebrew 清单，使用私有生成的 Brewfile，先 Bundle install --no-upgrade，再独立 check；依赖仍可能改变，没有自动回滚。Cargo 可在 Mac 按审核目录选择滚动版、精确版本或固定上游 Git HEAD 重部署；VS Code 扩展可从完整 Mac 清单逐项选取，精确版本或 Marketplace 当前版本重部署并回读默认稳定版配置文件。扩展具有与 VS Code 相同的权限且无法自动回滚；扩展部署本身不恢复编辑器设置或账号。另有独立的编辑器设置档案：支持 VS Code 稳定版、Insiders 与 Cursor 的默认用户配置，覆盖 macOS/Windows/Linux 的 21 项通用白名单 JSONC 合并，由用户逐项审核；不迁移工作区/命名 Profile、扩展账号、密钥或代理。NPM 可从已保存的 Mac 清单逐项恢复精确版本或自选 dist-tag；安装脚本关闭并回读核验。可单独导出选中 Windows 软件的原生 WinGet JSON，跨工具档案继续保留其他提供程序。完整范围见[环境备份说明](docs/MAC_BACKUP.md)与[成熟项目接入契约](docs/ENVIRONMENT_SOURCES.md)。

## Topgrade 环境迁移

环境配置页可独立备份 Topgrade 主配置、`topgrade.d/*.toml` 及明确添加的本地主目录内脚本/依赖文本文件。迁移包保留原文字节、相对目录和原可执行标记；载入后先查看原文，再逐文件选择并确认还原到一个全新目录。现有配置不会被覆盖，导入和还原不执行任何命令，Unix 上不恢复脚本执行位。包内可能包含本机路径与自定义命令，应保存在私人位置。此功能与软件清单备份独立，不会自动打包二进制、项目工作区、外部 include 或全部工具依赖。

命令行支持 `devenv-studio topgrade-backup --output /absolute/topgrade.json --include /absolute/script`（`--include` 可重复），以及 `devenv-studio topgrade-extract --input /absolute/topgrade.json --output /absolute/new-directory --all`；也可用重复的 `--file` 精选文件。新目录和新档案均拒绝覆盖。目标设备的 Topgrade、工具安装、主目录路径和自定义命令须另行审核，详见[重部署范围](docs/REDEPLOYMENT.md#topgrade-配置与脚本迁移包)。

迁移包也保留 fork 的自定义配置原文；重部署须核对来源仓库、固定提交和安装记录，版本号相同不代表实现兼容。本设备现用 Topgrade fork 的来源与资源附件边界见[重部署说明](docs/REDEPLOYMENT.md#fork-环境与安装来源)。本仓库已配置兼容依赖滚动策略，保留 Windows 绑定的配套版本约束。

## 开发环境重部署与渠道

环境配置页新增 Homebrew 的补齐、滚动更新和 HEAD 三种策略。HEAD 使用原生安装/更新命令并独立读取已安装渠道，不能只靠普通 Bundle 存在检查；稳定版链接冲突会停止。所有策略先保存基线与请求，不强制切换、清理或卸载。滚动更新使用当前本地配方元数据，不保证网络此刻最新版本。

同一页提供 macOS/Windows 共用的 Rust 工具链重部署：读取已安装渠道，选择 stable、beta、nightly、精确版本或按日期 nightly，保存/载入请求，审核后明确安装。使用标准位置现有 rustup 和已有官方默认工具链；安装 minimal profile、不自更新，独立核对安装身份、默认工具链未变及实际编译器版本/完整提交标识/日期，持久化结果后才显示成功。额外 targets、自定义组件、任意 Git URL 与首次 rustup 引导不在本轮范围。

所选请求可以导出原生 rust-toolchain.toml、Rust mise.toml、mise core 运行时请求与锁定结果，配合现有 Brewfile、WinGet 清单组成多个成熟项目的实际交换入口。mise core 仅管理经核对的运行时，不导入任意配置、插件或任务；锁文件不覆盖项目依赖和完整构建输入。

Python 工具区新增 macOS/Windows 共用的 uv 命令行工具重部署：读取 uv 清单，逐项选择固定版本或滚动最新版，保存/载入请求并回读安装结果。只管理所选独立 PyPI 命令工具；需要本机已有 uv 和 Python，pipx、项目依赖与 Python 运行时不自动导入。见[重部署契约](docs/REDEPLOYMENT.md)与[成熟项目接入取舍](docs/ENVIRONMENT_SOURCES.md)。

## 开发网络环境

新增网络环境页，可为下一次 GUI 部署保存“继承代理环境／直连／明确 HTTP 代理”和 npm、Python、Cargo 软件源。代理不含认证信息，软件源仅接受受限 HTTPS；启用前对照已启用内容与草稿，保存原值、记录历史并独立回读。支持 JSON 保存/载入、旧档案恢复，以及 Git、Cargo、npm、Pip、uv 原生配置导出；导出不覆盖用户现有工具配置。

网络方案实际接入本应用的 Homebrew、rustup、mise、uv、macOS Cargo，以及 Windows Cargo/npm/Pip/uv 部署。每次任务固定一份配置，传入真实参数与子进程环境，并写入部署证据；保持 npm 禁脚本、Pip wheel-only 等既有保护。它不修改系统代理、DNS、VPN、SSH、WinGet、DISM 或 VS Code 网络，也不提供网络隔离。官方 Cargo 请求仍可能受已有本地 source 配置影响；自定义 mirror 使用明确 source replacement。详见[网络配置范围与原生契约](docs/NETWORK.md)。

## 配置覆盖

| 区域 | 当前提供的实际选择 |
| --- | --- |
| 资源管理器 | 文件扩展名、隐藏文件、打开到此电脑/快速访问、紧凑/常规间距、最近文件与常用文件夹、项目复选框、同步提供程序通知、文件夹合并冲突提示 |
| 任务栏与开始菜单 | 搜索隐藏/图标/标签/搜索框、居左/居中对齐、任务视图、时钟秒数、右键结束任务、Windows 10 按钮合并方式、开始菜单推荐与启动追踪 |
| 外观与隐私 | 系统/应用深浅色、桌面此电脑、广告 ID、诊断个性化、反馈提示、欢迎与设置建议、设备设置完成提示；另有设备诊断数据策略、消费者体验、在线语音、活动发布/上传、Snap Assist、自动播放、窗口摇动和展开当前文件夹 |
| 软件部署 | 7-Zip、Firefox、Chrome、Git、VS Code、PowerToys、Terminal、PowerShell、Node.js LTS、LibreOffice、Everything、VLC 等 234 个明确 WinGet 包 |
| 开发环境 | 8个Cargo工具、4个uv工具、37个Pip包、6个NPM工具、8个VS Code扩展；按提供程序筛选，经同一执行器检查、安装并回查 |
| Windows 功能 | .NET 3.5、WSL 系统组件、虚拟机平台、Hyper-V、Windows 沙盒、打印为 PDF、工作文件夹 |
| 电源与节能 | 当前活动方案的插电显示器、睡眠、磁盘空闲、USB和PCIe节能参数；独立原值记录、回查与冲突保护恢复 |
| 存储与磁盘 | Windows系统卷/所有卷的分析与按介质优化；独立NTFS/ReFS删除通知策略检查，不修改通知策略 |
| 系统维护 | DISM 快速检查、完整扫描、组件存储修复，以及 SFC 只读验证 |

58 是可选方案数量，包含同一设置的不同取值，对应 40 组独立设置。各项按实际 LTSC 版本开放；同组的不同取值互斥。三个起步预设可显式载入并继续修改，不会自动应用。

设备级诊断数据策略和消费者体验策略采用HKLM，需要已有写入权限或管理员身份；实际行为受LTSC版本与组织策略约束，不代表已停止所有数据传输。其余新增桌面/隐私选项均单独选择。每项写入仍先记录原始值并回查；旧0.1–0.4恢复清单随程序保留，按日志原版本验证。

## 混合方案与迁移

Windows 配置可从方案工作区开始，提供日常桌面、Rust开发、Python工具、Python数据与交互、Web开发和WSL组件准备六种混合方案。每个组合同时列出设置、软件、工具或扩展；检查后明确加入当前选择，仍可逐项修改，再保存为同一份v3档案或导出完整方案。加入组合不会执行系统变更。

迁移页面可读取旧工具的SetupProfile 2.2.0或schema 1 Mac清单JSON，也可拖入文件。报告逐项区分已映射、需手动处理和未支持；初始不勾选，由用户决定哪些映射加入当前方案。只接受当前可信目录内的精确任务，保留其他选择并校验互斥设置。未迁移的私密字段不回显其值，原文件保持不变。Mac功能映射不保证相同版本、构建选项或二进制。

Windows 开发环境提供Cargo、uv、Pip、NPM和Microsoft VS Code五个实际执行适配器。计划先执行明确选中的WinGet软件，再执行开发任务，最后处理功能与维护；开发提供程序按任务重新查找，避免软件安装后仍沿用缺失状态。提供程序来自固定安装位置，不执行导入档案里的命令。Rust链接器/SDK、兼容Python和网络源需已准备好，uv不会自动下载Python。旧SetupProfile及Mac清单中的NPM包仅在当前目录有完全相同ID时映射；Pip按规范化发行包名匹配，37项中包含旧固定清单全部26项；Scoop、Cursor配置和其余系统调优仍保留未迁移说明。

目录来自固定WinUtil版本的全部234个有效WinGet包标识，排除了两个msstore来源项。软件实际版本、操作系统兼容性、硬件、许可和安装条件由执行时的来源与安装器检查；目录可选不等于已在LTSC设备验收。每次执行/检查最多128个部署任务，较大选择可保存/导出后拆分操作。

NPM仅使用官方Node.js安装中的node.exe与随附npm-cli.js，以固定的当前用户`%APPDATA%\npm`为前缀。检查和安装使用相同前缀与官方registry，不读取用户npm配置，不修改PATH、镜像或项目依赖，并禁用生命周期脚本。已安装包保留；结果核对精确包身份和版本，不能替代命令运行验收。目录中的全局工具适合独立使用，项目应继续锁定自己的依赖。较大的档案可以完整保存/导出；超过128个部署任务时，界面提示拆分并禁止启动执行。

## Pip 与 Python 数据环境

Pip任务使用已安装、注册的64位标准CPython 3.11–3.14，明确显示解释器候选路径，检查和安装前再核对运行时、用户目录与包身份。支持旧版26个固定Pip项目以及IPython、JupyterLab、pytest、Black、isort、mypy、Rich、Requests、HTTPX、Pydantic、NumPy和Pandas等共37项。原有Python工具组合保持不变；新增数据与交互组合明确选择Python 3.14、Git、VS Code及6个Pip项目，检查后才加入工作区。

安装使用当前用户范围、固定PyPI来源和兼容wheel，不自动升级、强制覆盖、构建源码或引导Python/pip。已安装且在该解释器中可见的所选发行包会跳过；结果核对同一解释器可见的精确包身份，不能替代导入或工具运行验收。依赖仍可能随安装调整，没有通用回滚，也不代替项目自己的虚拟环境和依赖锁定。Pip配置、项目、已激活环境和PATH不被改写；自定义Python路径/用户目录或虚拟环境会明确拒绝。详见[Pip运行范围与验证](docs/PIP.md)。

## 电源与节能

电源页提供五组16个AC选项，同组取值互斥。修改当前活动方案前捕获方案GUID与实际原值，持久化认证日志后写入，再重新应用同一方案并回查。电源恢复独立显示；若活动方案切换、参数被外部修改或原生调用失败，会停止并保留日志。它不创建卓越性能方案，不改电池DC参数或CPU/休眠设置。Windows硬件与组织策略可能限制实际效果；详见[电源范围与恢复](docs/POWER.md)。

新的v3工作区可一起保存系统设置、部署任务与电源选择，并继续读取v1/v2旧档案（电源为空）。旧工具创建/切换卓越性能方案的字段不会被自动替换为局部AC修改。

## 存储与磁盘

磁盘分析与优化各有系统卷、所有卷两个明确范围。系统卷在Windows执行时解析为真实卷GUID，不假定C盘；所有卷使用原生`defrag /C`，范围可能包含可移动卷。`/A`只进行分析，`/O`让Windows选择适当的介质优化方式，不自行推断SSD/HDD；所有操作默认低优先级、无并行参数。完成仅表示命令成功退出，不能证明磁盘健康、性能提升或每个卷都优化完成；优化没有通用撤销，结果与原始输出保存在执行历史。

页面的独立只读检查通过`fsutil behavior query DisableDeleteNotify`分别呈现NTFS/ReFS通知策略，未知或本地化未识别内容不会被推定为开启。删除通知开启不等于设备已执行TRIM。当前不改TRIM策略；旧档案的`enable_trim`仍需手动处理，`optimize_storage`只映射全部卷优化，不隐含启用TRIM。详见[存储范围与执行证据](docs/STORAGE.md)。

## 应用界面与偏好

界面参考自有旧工具的分层面板、导航选中态与卡片布局，统一浅色/深色下的标题、留白与控件样式。环境首页分开呈现清单、Rust与Homebrew；次要说明可展开，执行确认与关键限制保留在操作旁。设置、帮助、关于和来源固定在侧栏底部，工作区导航独立滚动；Windows选择栏只在相应工作区显示，并支持窄屏换行。设置提供系统/浅色/深色缩略图与五种实际强调色色块。

原生桌面提供简体中文和 English，可在应用设置中即时切换。设置、软件、功能、预设与方案中的用途说明也随语言切换；导出的执行方案保留可信清单原文，原始命令输出和底层诊断保持原样。

外观支持跟随系统、浅色、深色，以及蓝、青、紫、玫红、琥珀五种主题色；另有90%/100%/110%/125%界面缩放和舒适/紧凑密度。修改即时预览，点击保存后下次启动恢复。应用偏好与Windows配置档案分开；保存外观不会应用任何系统配置。偏好文件损坏或版本不支持时明确提示并保留原文件，不会悄悄覆盖。

状态中心列出实际主机与方案目标、执行能力、目录校验、WinGet能力、所选项目检查结果、后台任务和最近结果。部署历史显示持久记录及每项证据；未完成记录标为中断，不能作为恢复点。帮助页面提供操作步骤、组件前提与错误排查，关于页面提供版本、许可证及来源链接。

## 当前可以做什么

- 按类别浏览、搜索和逐项选择；分组展开详情，互斥选项自动替换同组选择，保留其他选择。
- 选择软件、系统功能和维护任务，与设置一起保存为配置档案或导出一个完整方案。
- 在 Windows 上检查选项当前状态、软件安装状态及功能状态；检查状态不会自动启动维护扫描。
- 切换 Windows 10 Enterprise LTSC 2021 / Windows 11 (IoT) Enterprise LTSC 2024 预览目标，检查适用性。
- 查看每个选项的影响、注册表地址、目标值、上游来源与固定版本。
- 保存和导入包含设置、部署与电源选择的 v3 配置档案；兼容旧版 v1/v2 档案。导出完整 JSON 计划，已有文件不会被覆盖。
- 在支持的 Windows 主机上，通过原生注册表接口应用；写入前记录原始值，写入后回读验证。
- 查看持久注册表恢复记录，恢复原值；兼容首版清单的历史记录，遇到外部修改冲突会停止。
- 软件安装和系统任务另存持久执行历史，写入前记意图、结束后检查结果；执行历史不冒充卸载或系统修复回滚。
- 后台任务支持协作式取消，保留已执行部分的恢复信息。

macOS与Windows均有本机开发清单采集入口，Mac可明确执行Homebrew Bundle配置；Windows配置部分在macOS/Linux仍为交互预览，不能修改Windows，也不会把预览结果显示为Windows系统检测结果。Windows 端按真实版本和版本编号检查兼容性；当前只开放 19044 与 26100 的 LTSC 客户端，不将普通家庭版、专业版或服务器当作已支持目标。

## 多项目如何融合

| 来源 | 本项目吸收的能力 | 整合方式 |
| --- | --- | --- |
| [Homebrew Bundle](https://github.com/Homebrew/brew) | Mac工具声明与配置 | 生成受限Brewfile，原生安装与独立检查；不执行任意导入脚本 |
| [rustup](https://github.com/rust-lang/rustup) | macOS/Windows Rust 渠道与固定版本重部署 | 原生安装、默认保护、工具链身份与实际 rustc 版本/提交回读 |
| [mise](https://github.com/jdx/mise) | 原生 Rust 工具链声明及 macOS/Windows 核心运行时重部署 | 受限 core 清单、精确/渠道请求、跨平台锁定、locked 安装与回读；不导入配置、插件或任务 |
| [WinGet](https://github.com/microsoft/winget-cli) | Windows软件身份、安装及原生清单交换 | 可信任务执行与选中软件的packages schema2.0导出 |
| [LTSC_Tools_Backup](https://github.com/nowaytouse/LTSC_Tools_Backup) | SetupProfile 迁移、Mac 清单映射、Cargo/uv/VS Code 工作流，以及选择与恢复设计 | 能力已映射到同一混合工作区；旧仓库已归档，历史与来源保留在本项目 |
| [WinUtil](https://github.com/ChrisTitusTech/winutil) | 固定版本的 234 个 WinGet 软件标识，以及拆分审查的隐私和桌面偏好 | 软件与配置进入同一份清单，由原生 Rust 引擎执行 |
| [Sophia Script](https://github.com/farag2/Sophia-Script-for-Windows) | 隐私、Explorer 和 Windows 版本适配参考 | 精确来源随配置记录，保留 MIT 声明 |
| [Win11Debloat](https://github.com/Raphire/Win11Debloat) | Explorer 与部分用户体验配置 | 只纳入可单独选择、可恢复的设置 |

程序不在线下载执行这些项目的脚本。它们的大型预设被拆解为可单独审查的配置；初版清单和许可证固定在仓库中。具体来源见 [来源与集成范围](docs/INTEGRATIONS.md) 和 [第三方声明](THIRD_PARTY_NOTICES.md)。

## 在 macOS 使用

需要 Rust 1.95 或更新版本；仓库默认使用 stable。GUI 使用 eframe/egui，业务与界面均为 Rust。

```sh
cargo run -p devenv-studio
```

本地打包需要 Python 3.11 或更新版本。需要应用包时，在完成构建后运行 `python3 scripts/package_macos.py --profile debug`。它首先生成 `DevEnv Studio 0.10.0.app`；同名产物已存在时，使用 `DevEnv Studio 0.10.0-preview-<UTC>.app` 保存新的预览，保留旧包。这个时间标签用于区分构建，不递增发布版本；源码提交标识用于追溯实际代码，签名和公证另行处理。

命令行也可以查看清单或生成计划：

```sh
cargo run -p devenv-studio -- catalog
cargo run -p devenv-studio -- tasks
cargo run -p devenv-studio -- power
cargo run -p devenv-studio -- providers
cargo run -p devenv-studio -- backup --output /absolute/path/mac-backup.json
cargo run -p devenv-studio -- migrate --target win11-ltsc-2024 --input examples/mac-development-backup.json
cargo run -p devenv-studio -- plan --target win11-ltsc-2024 --recipe python-data
cargo run -p devenv-studio -- storage-status
cargo run -p devenv-studio -- plan --profile examples/storage-workspace.json
cargo run -p devenv-studio -- plan --profile examples/power-workspace.json
cargo run -p devenv-studio -- plan --target win11-ltsc-2024 --recipe rust
cargo run -p devenv-studio -- migrate --target win11-ltsc-2024 --input examples/migration-profile.json
cargo run -p devenv-studio -- plan --target win11-ltsc-2024 --select explorer-extensions,privacy-advertising-id --tasks software-git-git,feature-netfx3
cargo run -p devenv-studio -- inspect --target win11-ltsc-2024 --select explorer-extensions
cargo run -p devenv-studio -- plan --profile my-profile.json --output my-plan.json
```

导出的计划只用于审查；执行时始终根据程序内置的可信清单重建方案。命令行不提供应用或恢复入口。

Linux 需要系统图形库和能显示中文的字体。Ubuntu 的编译依赖示例：

```sh
sudo apt-get install libxkbcommon-dev libwayland-dev libegl1-mesa-dev libfontconfig1-dev libfreetype6-dev
```

## Windows 使用范围

在目标日常账户中运行，先查看完整方案，再分别确认注册表配置或软件/系统任务。系统功能与维护需要已经提升的管理员进程；应用不会自行提权。软件安装器也可能请求管理员权限。

软件部署需要已经配置好的 Microsoft App Installer / WinGet、可用的软件源和网络。程序检查微软 App Installer 执行别名，使用精确包 ID，不搜索当前目录或任意 PATH 程序；不会自动下载引导脚本，也不会替用户接受来源或软件许可。遇到许可、源或安装失败会停止并保留历史，按官方 WinGet 提示处理后再检查。

功能启用先查询 DISM 状态，完成后再次查询；缺少组件、安装源或权限均明确失败。WSL 选项只启用系统组件，不承诺同时安装发行版、内核或 WSL 应用。功能可能要求虚拟化、重启或 Windows Update 提供组件源。维护任务显示命令结果与历史，不把退出码 0 当成系统健康或修复正确的证明。

恢复记录和部署执行历史保存到当前用户的应用数据目录。恢复针对记录过的注册表值，不是系统镜像，也不代表能够卸载应用或撤销所有系统操作。原来不存在的值会删除；执行时新建的空注册表键可能保留。重启资源管理器、注销或重启需求会显示在每个选项中，程序不会强制终止会话。

每个部署会话有 90 分钟上限，支持取消，并记录已完成和未验证的任务；安装器子进程或 Windows servicing 可能在取消后继续，程序不会把它们显示为已撤销。真实 LTSC 设备的安装、权限、注册表恢复与中断测试仍需单独验收。编译成功和模拟测试不等于设备验证。

## 开发与交付

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release --locked -p devenv-studio
```

工作区包含 `studio-core`（配置与计划）、`studio-platform`（注册表与恢复）、`studio-deployment`（软件、功能、维护及进程历史）和 `devenv-studio`（原生界面与命令行）。设计与首版边界见 [DESIGN.md](docs/DESIGN.md)，验证记录见 [VALIDATION.md](docs/VALIDATION.md)，版本选择与发布检查点见 [RELEASING.md](docs/RELEASING.md)。

Check 工作流在三种桌面系统上运行检查；Package desktop 工作流需手动触发，生成对应运行器平台的可执行文件及许可证，不自动发布 Release。当前仓库不附带未经验证的旧 EXE。

若 Actions 提示付款或支出限制、且任务没有任何步骤或日志，需由账户所有者在 [Billing & licensing](https://github.com/settings/billing) 检查付款状态和 Actions 预算；仓库代码及重跑任务不能解除该门禁。页面没有可处理问题时，可按 [GitHub 账户计费锁说明](https://docs.github.com/en/billing/how-tos/troubleshooting/locked-account) 联系 Support。`macos-latest` 的 ARM 容量排队通知是独立的服务提示；保留原生 ARM 检查与打包覆盖，不将其视作源码检查通过或失败。

与旧工具的实际功能差距见 [LEGACY_PARITY.md](docs/LEGACY_PARITY.md)，后续迁移顺序见 [ROADMAP.md](docs/ROADMAP.md)，变更记录见 [CHANGELOG.md](CHANGELOG.md)。采用 MIT 许可证；上游版权与许可证完整保留。
