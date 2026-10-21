# LTSC Studio

用 Rust 构建的 Windows LTSC 配置工作台。自己选择配置，先看清变更，再应用；macOS 与 Linux 可以使用同一个原生界面预览和保存方案。

这是一个新的独立项目，第一版把多项目中经过审查的配置行为统一为 Rust 清单、计划与恢复流程。现阶段覆盖隐私、资源管理器和桌面体验；完整的软件部署器还在后续范围中。

## 当前可以做什么

- 按意图浏览、搜索和逐项勾选配置，首次启动不勾选任何项目。
- 切换 Windows 10 Enterprise LTSC 2021 / Windows 11 (IoT) Enterprise LTSC 2024 预览目标，检查适用性。
- 查看每个选项的影响、注册表地址、目标值、上游来源与固定版本。
- 保存和导入用户选择的配置档案，导出完整 JSON 计划。已有文件不会被覆盖。
- 在支持的 Windows 主机上，通过原生注册表接口应用；写入前记录原始值，写入后回读验证。
- 查看持久恢复记录，恢复已记录的原值；遇到之后被其他程序修改的值会停止并报告冲突。
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

命令行也可以查看清单或生成计划：

```sh
cargo run -p ltsc-studio -- catalog
cargo run -p ltsc-studio -- plan --target win11-ltsc-2024 --select explorer-extensions,privacy-advertising-id
cargo run -p ltsc-studio -- plan --profile my-profile.json --output my-plan.json
```

导出的计划只用于审查；执行时始终根据程序内置的可信清单重建方案。命令行不提供应用或恢复入口。

Linux 需要系统图形库和能显示中文的字体。Ubuntu 的编译依赖示例：

```sh
sudo apt-get install libxkbcommon-dev libwayland-dev libegl1-mesa-dev libfontconfig1-dev libfreetype6-dev
```

## Windows 使用范围

在目标日常账户中运行，先查看计划，再在界面确认应用。当前首版配置均为用户偏好，优先不要求管理员权限。后续新增机器级设置时，权限不足会直接报告错误，不会默默跳过。

恢复记录保存到当前用户的应用数据目录。恢复针对记录过的注册表值，不是系统镜像，也不代表能够卸载应用或撤销所有系统操作。原来不存在的值会删除；执行时新建的空注册表键可能保留。重启资源管理器、注销或重启需求会显示在每个选项中，程序不会强制终止会话。

真实 LTSC 设备的应用、恢复、权限与中断测试仍需单独验收。编译成功和模拟测试不等于设备验证。

## 开发与交付

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release --locked -p ltsc-studio
```

工作区包含 `studio-core`、`studio-platform`、`ltsc-studio` 三部分。设计与首版边界见 [DESIGN.md](docs/DESIGN.md)，验证记录见 [VALIDATION.md](docs/VALIDATION.md)。

Check 工作流在三种桌面系统上运行检查；Package desktop 工作流需手动触发，生成对应运行器平台的可执行文件及许可证，不自动发布 Release。当前仓库不附带未经验证的旧 EXE。

后续迁移顺序见 [ROADMAP.md](docs/ROADMAP.md)，变更记录见 [CHANGELOG.md](CHANGELOG.md)。采用 MIT 许可证；上游版权与许可证完整保留。
