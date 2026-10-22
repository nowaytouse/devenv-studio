# 来源与集成范围

审查日期：2026-10-22。以下为源码行为审查，不是 LTSC 设备运行验收。当前清单版本仍为 `2026-10-22.1`：45 个注册表选项（31 组独立设置）、234 个 WinGet 软件包、20 个开发工具任务、7 个可选功能及 4 个维护任务；所有项目默认不选。

## 固定来源

- **[WinUtil](https://github.com/ChrisTitusTech/winutil)**：`7514faf0d0f4bff9b6c5d9213426ab52520cfac4`。拆分其配置清单中的独立注册表设置，保留来源；不运行上游捆绑脚本。 许可证：MIT。
- **[Sophia Script](https://github.com/farag2/Sophia-Script-for-Windows)**：`eeef532fc5f37129fe77ee58342f112f3bb71d39`。参考隐私、资源管理器及 LTSC 版本适配；仅整合已审查的单项设置。 许可证：MIT。
- **[Win11Debloat](https://github.com/Raphire/Win11Debloat)**：`a2d63c7c1b9c7cb6554d4fce3aa7f377567f7f9e`。整合可独立选择的 Explorer 与用户体验设置；不继承整个精简预设。 许可证：MIT。
- **[LTSC_Tools_Backup](https://github.com/nowaytouse/LTSC_Tools_Backup)**：`a8523abbe08147b4d5de8746f26d99d056057ab0`。沿用用户逐项选择、计划预览与先记录原值的设计经验。旧项目独立保留。 许可证：设计参考；未复制源码。

旧 LTSC_Tools_Backup 的提交用于设计来源与历史对应；未复制其源文件或个人工具清单。旧仓库存在未提交改动，保持原样。当前融合发生在同一配置清单、计划和 Rust 执行引擎中，而不是启动三个上游脚本。

## 首批映射与后续扩展

下表保留首批设置的对应关系。扩展后的全部选项、互斥组和预设以 [catalog.json](../data/catalog.json) 为准，每项都保留精确来源。新版涵盖 Explorer 打开位置、视图密度、快速访问记录、复选框、任务栏搜索模式、对齐、时钟、结束任务、按钮合并、主题、开始菜单和推荐偏好。

软件清单见 [deployment.json](../data/deployment.json)：234 个唯一的软件包 ID 对应 WinUtil 固定提交中全部符合项目 ID 字符规则的原生 WinGet 条目（保留原有 30 项并新增 204 项）。两个 `msstore:` 条目使用 Microsoft Store 源标识，未纳入原生 WinGet 包目录。新增的 204 项各自链接到 WinUtil JSON 中对应源键行，原有 30 项的任务 ID、包 ID 与既有证据保持不变；执行时使用已安装的官方 WinGet 与精确 ID。实际安装版本由执行时来源决定，不固定，也不保证跨设备版本一致。固定文件 SHA-256 为 `5072ff3f3b4c689db313000c8ef7ff93c655f5cea55c5990192ead87789ecf41`；`scripts/update_catalog.py --source <本地固定源文件> --check` 可用已保存的源文件离线校验摘要、目录 ID 与英文覆盖率，不写入文件。

开发工具包含 8 个 Cargo crate、4 个 uv 工具和 8 个 VS Code 扩展。它们是独立的用户级任务，需有对应工具链或编辑器；如果同时选择目录中提供的 WinGet 前置软件，执行引擎会先安装所选软件，再运行开发工具任务。Cargo/PyPI/Marketplace 的版本取执行时来源，不固定。Cargo 构建、Python 包安装后端或 VS Code 扩展可能执行其发布者提供的代码；用户应先审阅来源、许可与权限。本功能不提升这些任务的权限，也不写入系统 PATH。各任务链接对应的 crates.io、PyPI 或 Marketplace 项目页。

7 项 Windows 功能通过本机 DISM 查询、启用和回读，4 项维护通过 DISM/SFC 运行固定参数。相关微软文档链接保存在每项证据中；没有将 PowerShell 上游整体包装为 Rust 功能。

首版的不可变清单副本保存在 [catalog-2026-10-21.1.json](../data/catalog-2026-10-21.1.json)，仅用于验证其历史恢复记录，不能借导入文件扩张执行权限。

## 首批逐项映射

| 配置 ID | 名称 | 适用目标 | 精确来源 |
| --- | --- | --- | --- |
| `privacy-advertising-id` | 关闭应用广告 ID | LTSC 2021 / 2024 | [源码 1](https://github.com/ChrisTitusTech/winutil/blob/7514faf0d0f4bff9b6c5d9213426ab52520cfac4/config/tweaks.json)，[源码 2](https://github.com/farag2/Sophia-Script-for-Windows/blob/eeef532fc5f37129fe77ee58342f112f3bb71d39/src/Sophia_Script_for_Windows_10_LTSC_2021/Module/Sophia.psm1)，[源码 3](https://github.com/farag2/Sophia-Script-for-Windows/blob/eeef532fc5f37129fe77ee58342f112f3bb71d39/src/Sophia_Script_for_Windows_11_LTSC_2024/Module/Sophia.psm1) |
| `privacy-tailored-experiences` | 关闭诊断数据个性化体验 | LTSC 2021 / 2024 | [源码 1](https://github.com/ChrisTitusTech/winutil/blob/7514faf0d0f4bff9b6c5d9213426ab52520cfac4/config/tweaks.json)，[源码 2](https://github.com/farag2/Sophia-Script-for-Windows/blob/eeef532fc5f37129fe77ee58342f112f3bb71d39/src/Sophia_Script_for_Windows_10_LTSC_2021/Module/Sophia.psm1)，[源码 3](https://github.com/farag2/Sophia-Script-for-Windows/blob/eeef532fc5f37129fe77ee58342f112f3bb71d39/src/Sophia_Script_for_Windows_11_LTSC_2024/Module/Sophia.psm1) |
| `privacy-feedback` | 减少反馈请求 | LTSC 2021 / 2024 | [源码 1](https://github.com/ChrisTitusTech/winutil/blob/7514faf0d0f4bff9b6c5d9213426ab52520cfac4/config/tweaks.json)，[源码 2](https://github.com/farag2/Sophia-Script-for-Windows/blob/eeef532fc5f37129fe77ee58342f112f3bb71d39/src/Sophia_Script_for_Windows_10_LTSC_2021/Module/Sophia.psm1)，[源码 3](https://github.com/farag2/Sophia-Script-for-Windows/blob/eeef532fc5f37129fe77ee58342f112f3bb71d39/src/Sophia_Script_for_Windows_11_LTSC_2024/Module/Sophia.psm1) |
| `explorer-extensions` | 显示文件扩展名 | LTSC 2021 / 2024 | [源码 1](https://github.com/Raphire/Win11Debloat/blob/a2d63c7c1b9c7cb6554d4fce3aa7f377567f7f9e/Regfiles/Show_Extensions_For_Known_File_Types.reg)，[源码 2](https://github.com/farag2/Sophia-Script-for-Windows/blob/eeef532fc5f37129fe77ee58342f112f3bb71d39/src/Sophia_Script_for_Windows_10_LTSC_2021/Module/Sophia.psm1)，[源码 3](https://github.com/farag2/Sophia-Script-for-Windows/blob/eeef532fc5f37129fe77ee58342f112f3bb71d39/src/Sophia_Script_for_Windows_11_LTSC_2024/Module/Sophia.psm1) |
| `explorer-hidden` | 显示隐藏文件 | LTSC 2021 / 2024 | [源码 1](https://github.com/Raphire/Win11Debloat/blob/a2d63c7c1b9c7cb6554d4fce3aa7f377567f7f9e/Regfiles/Show_Hidden_Folders.reg)，[源码 2](https://github.com/farag2/Sophia-Script-for-Windows/blob/eeef532fc5f37129fe77ee58342f112f3bb71d39/src/Sophia_Script_for_Windows_10_LTSC_2021/Module/Sophia.psm1)，[源码 3](https://github.com/farag2/Sophia-Script-for-Windows/blob/eeef532fc5f37129fe77ee58342f112f3bb71d39/src/Sophia_Script_for_Windows_11_LTSC_2024/Module/Sophia.psm1) |
| `experience-taskview` | 隐藏任务视图按钮 | LTSC 2021 / 2024 | [源码 1](https://github.com/ChrisTitusTech/winutil/blob/7514faf0d0f4bff9b6c5d9213426ab52520cfac4/config/tweaks.json) |
| `experience-search` | 隐藏任务栏搜索控件 | LTSC 2021 / 2024 | [源码 1](https://github.com/ChrisTitusTech/winutil/blob/7514faf0d0f4bff9b6c5d9213426ab52520cfac4/config/tweaks.json)，[源码 2](https://github.com/farag2/Sophia-Script-for-Windows/blob/eeef532fc5f37129fe77ee58342f112f3bb71d39/src/Sophia_Script_for_Windows_10_LTSC_2021/Module/Sophia.psm1)，[源码 3](https://github.com/farag2/Sophia-Script-for-Windows/blob/eeef532fc5f37129fe77ee58342f112f3bb71d39/src/Sophia_Script_for_Windows_11_LTSC_2024/Module/Sophia.psm1) |
| `experience-left-taskbar` | 任务栏图标靠左 | LTSC 2024 | [源码 1](https://github.com/ChrisTitusTech/winutil/blob/7514faf0d0f4bff9b6c5d9213426ab52520cfac4/config/tweaks.json) |
| `privacy-app-launch-tracking` | 关闭应用启动追踪 | LTSC 2021 / 2024 | [源码 1](https://github.com/ChrisTitusTech/winutil/blob/7514faf0d0f4bff9b6c5d9213426ab52520cfac4/config/tweaks.json)，[源码 2](https://github.com/Raphire/Win11Debloat/blob/a2d63c7c1b9c7cb6554d4fce3aa7f377567f7f9e/Regfiles/Disable_Telemetry.reg) |
| `experience-welcome` | 关闭更新后的欢迎推荐 | LTSC 2024 | [源码 1](https://github.com/farag2/Sophia-Script-for-Windows/blob/eeef532fc5f37129fe77ee58342f112f3bb71d39/src/Sophia_Script_for_Windows_11_LTSC_2024/Module/Sophia.psm1#L784)，[源码 2](https://github.com/Raphire/Win11Debloat/blob/a2d63c7c1b9c7cb6554d4fce3aa7f377567f7f9e/Regfiles/Disable_Windows_Suggestions.reg) |
| `experience-tips` | 减少 Windows 使用提示 | LTSC 2024 | [源码 1](https://github.com/farag2/Sophia-Script-for-Windows/blob/eeef532fc5f37129fe77ee58342f112f3bb71d39/src/Sophia_Script_for_Windows_11_LTSC_2024/Module/Sophia.psm1#L835) |
| `experience-settings-suggestions` | 关闭设置中的建议内容 | LTSC 2024 | [源码 1](https://github.com/farag2/Sophia-Script-for-Windows/blob/eeef532fc5f37129fe77ee58342f112f3bb71d39/src/Sophia_Script_for_Windows_11_LTSC_2024/Module/Sophia.psm1#L886)，[源码 2](https://github.com/Raphire/Win11Debloat/blob/a2d63c7c1b9c7cb6554d4fce3aa7f377567f7f9e/Regfiles/Disable_Windows_Suggestions.reg) |

注册表完整地址与目标值见 [catalog.json](../data/catalog.json)，应用内的变更方案也会逐项列出。WinUtil 的任务栏 Toggle 在这里被映射为独立的隐藏/靠左选项，对应其数据中的反向状态；不沿用上游预设的固定“原值”，而是在执行时保存实际原值。

Windows 11 的内容推荐选项依据 Sophia 的 LTSC 2024 模块纳入，只对 2024 目标开放。没有把相同注册表路径直接推定为所有旧 LTSC 都具备相同 UI。任务栏设置需在真实设备确认对应界面有效。

## 行为差异与边界

- WinUtil 的 Telemetry 预设还包含 Defender 样本提交、诊断服务、错误报告与 PowerShell 环境变量等操作，本项目只拆取明确列出的用户偏好。
- Sophia 的部分函数会先删除已有组织策略以让系统 UI 可调。本项目不删除这些策略。已有组织策略可能覆盖用户偏好；回读成功只证明保存的注册表值匹配，实际界面效果需要在目标系统确认。
- Win11Debloat 的建议内容和 Telemetry 文件覆盖多组设置。本项目仅映射清单中列出的值，不执行整份 REG 文件，不移除诊断、安全或更新服务。
- “显示隐藏文件”只写入 `Hidden=1`，不修改 `ShowSuperHidden`；用户已有的受保护文件显示偏好保持其原值。
- 不复制上游的组件移除、包卸载、ACL 修改、默认值式撤销和强制重启行为。
- 后续新来源或新版本必须经过人工审查、定向测试与许可复核，不在运行时自动更新执行清单。
