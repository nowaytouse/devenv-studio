# LTSC Workspace

> **已退役：后续维护统一移至 [DevEnv Studio](https://github.com/nowaytouse/devenv-studio)。** 新项目同时面向 macOS 与 Windows 开发环境；该仓库目前为私有，未获访问权限的用户可能无法打开。旧仓库保留源码、历史、内置清单和旧 EXE 供追溯，不再独立新增功能。退役依据、产物来源与兼容性边界见 [ARCHIVED.md](ARCHIVED.md)。GitHub 只读归档在新主仓整合提交推送后执行；本文档不表示所有功能已经等价迁移或完成 Windows/LTSC 实机验收。

## 旧版本历史使用说明

以下内容描述旧 LTSC Workspace 的使用与限制；新版本请以 DevEnv Studio 的文档和实际迁移报告为准。

给个人设备用的 Windows LTSC 部署器。源 Mac 提供工具清单；Windows 程序按映射补齐软件和开发工具。网络、存储、电源、Windows 功能等系统调优是另外的显式选择，不会因为“同步工具”而自动执行。

Windows 端是 Rust 编译的桌面程序，没有 WebView、Electron 或网页服务。部署流程、清单处理、超时与取消、回滚记录都在 Rust 中；WinGet、DISM 等系统工具以独立参数调用。Scoop 上游本身使用 PowerShell，本项目不再附带或执行部署用的外置 PS1。

## 日常使用

源 Mac 更新工具后，在**干净的仓库**里运行一次：

```text
cargo run --bin macos_inventory -- --sync
```

程序会先快进拉取、采集工具、校验 Windows 映射，再仅提交清单并推送；采集不完整、出现未映射工具、工作区不干净或远端不同步都会报错停止。普通采集命令仍可用于本地预览，不会自动推送。清单只保留工具名称，不记录用户名、路径或私有 tap。

Windows 清单来源支持以下方式（根目录 `.exe` 已替换为源提交 `3fd48ac` 的成功 CI 产物；后续源码改动不代表这个产物也已更新）：

- 默认先访问 GitHub Raw；连接失败或清单内容无效时，自动通过 [GitHub Contents API](https://docs.github.com/en/rest/repos/contents#get-repository-content) 获取同一份清单。**每个来源**连接最多 8 秒、下载最多 20 秒，外层进程等待上限 25 秒；不是两次尝试合计 25 秒。只允许 HTTPS 重定向，不降低证书验证。
- 可填写可访问的 HTTPS 清单地址。自定义来源只使用该地址，不会再切换到官方来源；点击更新后地址保存在 `%LOCALAPPDATA%\LTSCWorkspace\inventory_source_url.txt`，清单缓存位于同目录。
- 可输入本地 JSON 路径，导入经 U 盘或内网传来的清单，读取上限为 1 MiB。在线和本地来源都经过 JSON、工具映射及当前版本兼容性校验；失败时保留上一份有效清单。

官方 API 也可能被阻断或触发访问限额，因此它不是保证可达的镜像。两个官方来源都不可用时，可换用自己信任且可访问的 HTTPS 地址，或从其他设备导出 JSON 后本地导入；首次启动也可使用内置清单。单纯更新 Mac 清单**不需要重新构建 Windows 程序**，但新增工具要先在仓库建立 Windows 映射并发布新程序。

清单可离线使用，但安装软件仍需网络：首次引导 WinGet 会访问 GitHub API 和微软的 GitHub Release 文件，Scoop 会访问 GitHub 仓库和软件发布地址，其他包管理器也需要访问各自的软件源。

默认模式是“同步 Mac 工具”：只安装有映射的工具及必要运行时，不夹带软件大全或系统调优。需要更多软件时勾选“附加推荐软件”；需要调优时再单独选择相应任务。不能自动安装的项目会显示为手动项，未知项目会阻止部署，避免静默漏装。

程序使用 UAC 管理员清单。若 Windows 日常账户是标准用户，并用**另一个管理员账户**批准 UAC，按用户安装的软件会进入管理员账户；此场景尚未实现跨账户部署，请在目标账户上运行并核对结果。

## 失败与撤销

每个外部命令有超时和取消；部署结束会明确显示成功、部分失败、取消或异常。若清单更新超时或失败，程序保留上一份有效缓存；也可改用本地 JSON 导入，或首次使用内置清单。首次缺少 WinGet 时，程序下载同一个微软稳定发布中的 MSIX 和 x64 依赖，用 Windows 部署接口安装到当前账户，再验证 `winget.exe`。下载失败、系统不支持或别名不可用都会报告为失败，不会当作完成。

“撤销上次调优”恢复本程序记录的注册表、TCP、TRIM 与电源原值。它**不是系统镜像**，不卸载软件，也不自动关闭已启用的 Windows 功能。存储优化使用 Windows 的按介质策略；极限电源档只调整接通电源参数。建议先预览计划，再对系统调优逐项启用。

## 维护与验证

改动记录见 [Changelog](Docs/CHANGELOG.md)，包含尚未发布的修改及适用限制。

代码与交付规范见 [AGENTS.md](AGENTS.md)。当前程序来源、文件摘要、已验证范围和 LTSC 试用步骤见 [Windows 验证状态](Docs/READINESS.md)；工具名称同步不等于 HEAD/nightly 版本或 FFmpeg 编译选项完全一致。

工具映射位于 [parity_rules.json](src/assets/parity_rules.json)，可安装清单位于 [setup_profile.json](src/assets/setup_profile.json)。Windows CI 会运行单元测试、Rust 静态检查、在线软件源审计，并生成 x64 发布程序；CI 产物通过后才应替换仓库根目录的 `.exe`。macOS 上的交叉编译只能证明 Windows 代码可编译，不能代替真实 LTSC 2019/2021/2024 的安装与回滚验收。

本项目不会为了“加速”删除系统组件或套用不可逆的 debloat 清单。LTSC 版本、商店依赖、网络和硬件能力不同，某些项目仍可能需要设备上人工验证。
