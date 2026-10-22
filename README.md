# LTSC Studio

用 Rust 构建的 Windows LTSC 配置工作台。自己选择配置，先看清变更，再应用；macOS 与 Linux 可以使用同一个原生界面预览和保存方案。

当前为 0.3.0：将多个上游的配置和软件选择整合为同一套 Rust 清单、配置档案、计划与执行流程。系统设置提供 45 个选项，覆盖 31 组独立设置；另有 30 个软件包、7 个 Windows 可选功能和 4 个维护任务。所有选择默认关闭。

## 配置覆盖

| 区域 | 当前提供的实际选择 |
| --- | --- |
| 资源管理器 | 文件扩展名、隐藏文件、打开到此电脑/快速访问、紧凑/常规间距、最近文件与常用文件夹、项目复选框、同步提供程序通知、文件夹合并冲突提示 |
| 任务栏与开始菜单 | 搜索隐藏/图标/标签/搜索框、居左/居中对齐、任务视图、时钟秒数、右键结束任务、Windows 10 按钮合并方式、开始菜单推荐与启动追踪 |
| 外观与隐私 | 系统/应用深浅色、桌面此电脑、广告 ID、诊断个性化、反馈提示、欢迎与设置建议、设备设置完成提示 |
| 软件部署 | 7-Zip、Firefox、Chrome、Git、VS Code、PowerToys、Terminal、PowerShell、Node.js LTS、LibreOffice、Everything、VLC 等 30 个明确 WinGet 包 |
| Windows 功能 | .NET 3.5、WSL 系统组件、虚拟机平台、Hyper-V、Windows 沙盒、打印为 PDF、工作文件夹 |
| 系统维护 | DISM 快速检查、完整扫描、组件存储修复，以及 SFC 只读验证 |

45 是可选方案数量，包含同一设置的不同取值，不代表 45 组独立功能。各项按实际 LTSC 版本开放；同组的不同取值互斥。三个起步预设可显式载入并继续修改，不会自动应用。

## 应用界面与偏好

原生桌面提供简体中文和 English，可在应用设置中即时切换。设置、软件、功能、预设与方案中的用途说明也随语言切换；导出的执行方案保留可信清单原文，原始命令输出和底层诊断保持原样。

外观支持跟随系统、浅色、深色，以及蓝、青、紫、玫红、琥珀五种主题色；另有90%/100%/110%/125%界面缩放和舒适/紧凑密度。修改即时预览，点击保存后下次启动恢复。应用偏好与Windows配置档案分开；保存外观不会应用任何系统配置。偏好文件损坏或版本不支持时明确提示并保留原文件，不会悄悄覆盖。

状态中心列出实际主机与方案目标、执行能力、目录校验、WinGet能力、所选项目检查结果、后台任务和最近结果。部署历史显示持久记录及每项证据；未完成记录标为中断，不能作为恢复点。帮助页面提供操作步骤、组件前提与错误排查，关于页面提供版本、许可证及来源链接。

## 当前可以做什么

- 按类别浏览、搜索和逐项选择；分组展开详情，互斥选项自动替换同组选择，保留其他选择。
- 选择软件、系统功能和维护任务，与设置一起保存为配置档案或导出一个完整方案。
- 在 Windows 上检查选项当前状态、软件安装状态及功能状态；检查状态不会自动启动维护扫描。
- 切换 Windows 10 Enterprise LTSC 2021 / Windows 11 (IoT) Enterprise LTSC 2024 预览目标，检查适用性。
- 查看每个选项的影响、注册表地址、目标值、上游来源与固定版本。
- 保存和导入包含设置与部署任务的 v2 配置档案；兼容旧版 v1 设置档案。导出完整 JSON 计划，已有文件不会被覆盖。
- 在支持的 Windows 主机上，通过原生注册表接口应用；写入前记录原始值，写入后回读验证。
- 查看持久注册表恢复记录，恢复原值；兼容首版清单的历史记录，遇到外部修改冲突会停止。
- 软件安装和系统任务另存持久执行历史，写入前记意图、结束后检查结果；执行历史不冒充卸载或系统修复回滚。
- 后台任务支持协作式取消，保留已执行部分的恢复信息。

macOS/Linux 是实际可交互的预览模式，不能修改 Windows，也不会把预览结果显示为系统检测结果。Windows 端按真实版本和版本编号检查兼容性；当前只开放 19044 与 26100 的 LTSC 客户端，不将普通家庭版、专业版或服务器当作已支持目标。

## 多项目如何融合

| 来源 | 本项目吸收的能力 | 整合方式 |
| --- | --- | --- |
| [LTSC_Tools_Backup](https://github.com/nowaytouse/LTSC_Tools_Backup) | 用户选择、计划预览、记录原值与回滚的设计经验 | 新 Rust 核心重新实现；旧仓库独立保留 |
| [WinUtil](https://github.com/ChrisTitusTech/winutil) | 经过拆分审查的隐私和桌面偏好 | 配置映射进入同一份清单，由原生 Rust 引擎执行 |
| [Sophia Script](https://github.com/farag2/Sophia-Script-for-Windows) | 隐私、Explorer 和 Windows 版本适配参考 | 精确来源随配置记录，保留 MIT 声明 |
| [Win11Debloat](https://github.com/Raphire/Win11Debloat) | Explorer 与部分用户体验配置 | 只纳入可单独选择、可恢复的设置 |

程序不在线下载执行这些项目的脚本。它们的大型预设被拆解为可单独审查的配置；初版清单和许可证固定在仓库中。具体来源见 [来源与集成范围](docs/INTEGRATIONS.md) 和 [第三方声明](THIRD_PARTY_NOTICES.md)。

## 在 macOS 预览

需要 Rust 1.95 或更新版本；仓库默认使用 stable。GUI 使用 eframe/egui，业务与界面均为 Rust。

```sh
cargo run -p ltsc-studio
```

本地需要应用包时，在完成构建后运行 `python3 scripts/package_macos.py --profile debug`。它生成带版本号的开发预览包，不覆盖已有版本；签名和公证另行处理。

命令行也可以查看清单或生成计划：

```sh
cargo run -p ltsc-studio -- catalog
cargo run -p ltsc-studio -- tasks
cargo run -p ltsc-studio -- plan --target win11-ltsc-2024 --select explorer-extensions,privacy-advertising-id --tasks software-git-git,feature-netfx3
cargo run -p ltsc-studio -- inspect --target win11-ltsc-2024 --select explorer-extensions
cargo run -p ltsc-studio -- plan --profile my-profile.json --output my-plan.json
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
cargo build --release --locked -p ltsc-studio
```

工作区包含 `studio-core`（配置与计划）、`studio-platform`（注册表与恢复）、`studio-deployment`（软件、功能、维护及进程历史）和 `ltsc-studio`（原生界面与命令行）。设计与首版边界见 [DESIGN.md](docs/DESIGN.md)，验证记录见 [VALIDATION.md](docs/VALIDATION.md)。

Check 工作流在三种桌面系统上运行检查；Package desktop 工作流需手动触发，生成对应运行器平台的可执行文件及许可证，不自动发布 Release。当前仓库不附带未经验证的旧 EXE。

与旧工具的实际功能差距见 [LEGACY_PARITY.md](docs/LEGACY_PARITY.md)，后续迁移顺序见 [ROADMAP.md](docs/ROADMAP.md)，变更记录见 [CHANGELOG.md](CHANGELOG.md)。采用 MIT 许可证；上游版权与许可证完整保留。
