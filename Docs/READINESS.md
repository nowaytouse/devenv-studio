# Windows 使用与验证状态

核查日期：2026-10-20。当前状态是**已有通过 CI 的 Windows 试用程序，但尚未完成真实 LTSC 设备验收**，不宣称“全部完善”或“任何 LTSC 都即拿即用”。

## 2026-10-20 命令输出可靠性与 CI 复核

按用户请求单次检查 [CI 39227504057](https://github.com/nowaytouse/LTSC_Tools_Backup/actions/runs/39227504057)：提交 `5f79a46ddf5338a732d32e1559b397b8ea91999a` 的 Windows、macOS、依赖审计均成功，包括 Windows 在线软件源审计、发布构建和独立产物检查。该结果不覆盖后续 `bc6cbcd` 或本节源码；推送后的新 CI 不在本轮自动查询或等待。

本轮修复输出读取失败被误当作空输出的问题。命令只有正常退出且输出读取完整，才可以按退出码判断成功；读取错误、线程断开、管道缺失和输出等待超时会明确失败。实际读错时保留部分输出和分管道诊断，读取中断会重试，正常 EOF 的空输出不被误判失败。两条管道共用两秒收尾期限；后代进程一直持有输出管道时，调用方会报错返回，但阻塞读取线程可能要等管道关闭才退出，尚未实现跨平台可取消读取。

保留已有 cc 1.9.0、find-msvc-tools 0.1.15、tokio 1.54.0 更新。单独升级 windows-collections 0.100.0 的组合已复现六项 Windows 类型错误，现保持 windows 0.62.2 / windows-collections 0.3.2 配套。根目录 EXE 未更新，仍以“已确认的产物”一节为准。

本地 Rust stable 1.99.0 的全部测试为 68 项通过、4 个子进程辅助项按原设计忽略，格式和零警告静态检查通过；Windows GNU 的全部目标编译检查和零警告静态检查通过，交叉目标使用本机 nightly。最新 RustSec 数据库仍为下节记录的 `999ec89d`，扫描 366 项锁文件依赖，零项已知漏洞、三条既有维护提示，未忽略公告。本地和交叉目标检查不代替新提交的 Windows MSVC/stable CI，也不代替 LTSC 设备验收。

## 2026-10-19 可靠性加固与依赖更新

本轮源码改为使用独占暂存文件保存清单与回滚账本，保存失败不会推进回滚记录、完成或已恢复状态。软件源审计对工作线程异常退出和未完成检查显式报错，避免漏审后的假通过；这些失败路径有离线回归测试。文件同步与原子替换降低部分写入风险，但不保证断电后目录元数据持久化，也不代替真实 Windows 文件锁、权限和杀毒软件干预场景验证。

依赖采用当前核验的最新兼容稳定版本，包括 eframe 0.37.0；`windows-collections` 保持 0.3.2，因为 0.100.0 不属于 `windows` 0.62.2 的同一投影类型家族。没有为了版本号更大而引入已知不兼容组合。

本地验证：Rust stable 1.99.0 下全部测试 58 项通过、4 个子进程辅助项按原设计忽略，格式及零警告静态检查通过；Windows GNU 目标的全部目标编译检查及零警告静态检查通过，后者使用本机 nightly，不等同于 Windows MSVC/stable CI 或设备执行。

RustSec 数据库核验至 `999ec89d5d3f10f20be14750ec8c2d8b44e0ac9b`：366 项锁文件依赖未发现已知漏洞，但有 [zvariant](https://rustsec.org/advisories/RUSTSEC-2026-0370)、[zbus_names](https://rustsec.org/advisories/RUSTSEC-2026-0371) 和 [zvariant_derive](https://rustsec.org/advisories/RUSTSEC-2026-0372) 三条停止独立维护提示。这些 crate 已合并到 zbus 6，不存在原 crate 的修复版本。当前条目由无障碍支持的 Linux AT-SPI 间接依赖引入跨平台锁文件，不在本项目 Windows 构建的有效依赖树中；本项目也不构建 Linux/macOS GUI。保留提示并等待上游兼容迁移，不强行替换不兼容主版本、不移除无障碍支持，也没有设置忽略公告。

本轮没有查询、监控或重跑 CI，也没有更新根目录 EXE。以下旧产物的来源与设备验收边界继续有效；本轮源码改动不能计入旧 EXE 的能力。

## 2026-10-13 软件源审计修复

[CI 38240426878](https://github.com/nowaytouse/LTSC_Tools_Backup/actions/runs/38240426878) 的唯一失败项为 Scoop `jhead`，三个候选清单均返回 404。官方在 [2026-10-05 撤销该清单](https://github.com/ScoopInstaller/Main/commit/b013317d258c42bb7a06671c2421c0ed14b4cb0f)，不是本项目 raw 下载回退失效。

Mac 清单继续保留 `jhead`；Windows 明确列为手动源码构建项。[上游说明](https://github.com/Matthias-Wandel/jhead/blob/master/readme.txt) 使用 Visual C++ 2022 和 `make.bat`，未核验任何可自动安装的 Windows 发布归档。它不计作已安装，也没有用 ExifTool 冒充等价实现。此次映射修复已通过本轮核对的 `5f79a46` CI，但尚未进入下述根目录 EXE；真实 LTSC 构建运行仍待验证。

## 已确认的产物

根目录 `ltsc_setup_gui.exe` 来自 [CI 37089825898](https://github.com/nowaytouse/LTSC_Tools_Backup/actions/runs/37089825898)，而不是在 Mac 上构建的程序。

| 项目 | 证据 |
| --- | --- |
| 源提交 | `3fd48acec93650f885bd235073d9bb3abb8fe486` |
| GitHub 产物 | `ltsc_setup_gui-windows-x64`，ID `11262586509` |
| EXE SHA-256 | `b1f783bad5c0ba3f8ee8e54e309bd05558e1a61d4060be17e8514ed46186505c` |
| CI | macOS 检查、Windows 测试/静态检查/在线软件源审计/发布构建、依赖审计均成功 |
| 本地文件复核 | Windows x64 GUI PE、包含 `requireAdministrator` manifest、导入表未发现动态 `VCRUNTIME*.dll` |

该产物包含 Raw → 官方 API 回退、自定义 HTTPS 和本地 JSON 清单导入。**2026-10-04 的 Scoop 修复、依赖和清单映射更新属于后续源码，不在这个 EXE 中。** 需新提交的 Windows CI 成功并替换产物后，才可把它们计入二进制能力。

旧产物遇到本次新增映射的清单会拒绝替换缓存并提示更新程序，这是兼容性保护，不是清单同步已完成。若要使用本次新增工具，应等待包含新映射的 Windows 构建，而不是反复对旧 EXE 点击清单更新。

上述检查没有在真实 Windows 设备启动 GUI，也没有在 LTSC 上执行软件安装或回滚。

## 仍然存在的边界

- 清单是工具名称与功能映射，不是 macOS 环境镜像。没有记录全部版本、安装通道、环境变量、凭据或 FFmpeg 构建 flags；HEAD/nightly 和编译能力一致性仍需专项核验。
- 清单可离线导入；WinGet 引导、Scoop Git 仓库和软件包下载仍需要各自网络。官方 API 有访问限额，备用接口不保证可达。
- 标准用户通过另一管理员账户批准 UAC 时，按用户安装的工具会落入管理员账户；未实现跨账户部署。
- GUI、驱动、网络、包管理器、系统功能和调优效果受 LTSC 版本及设备影响。现代 Windows CI 不是 LTSC 2019/2021/2024 兼容性认证。
- 调优撤销只恢复本程序记账的变更，不是系统备份，不卸载软件，也不自动关闭已启用的 Windows 功能。
- 本次源码修复已有 Git 但 WinGet 不可用时的 Scoop 引导，以及自定义 `SCOOP` 根目录的命令查找；回归检查不代替真实安装验收。
- IDE/Agent 内置资源没有逐项完成 Windows 运行验证。静态扫描发现旧 `googlecloudtools.datacloud_telemetry/hooks.json` 使用 Mac 绝对路径，当前迁移不重写它；此问题未修复，不能把资源成功释放当作插件可用。它不在默认“开发工具”任务的执行路径中。

## 本机工具刷新

2026-10-04 重新采集 160 项工具，覆盖 112 个 Homebrew formula、13 个 cask、29 个 Cargo、3 个 NPM 与 3 个 UV 工具。2026-10-13 调整 `jhead` 后，名称级映射中有 115 项自动、8 项内置/替代、9 项 Mac 专属和 28 项手动，没有未映射项；**手动项不等于已经安装**。

新增 `crane`、`gradle`、`libngtcp2`、`lld@21`、`pinact`、`ui-ux-pro-max-cli`。Gradle 的安装计划包含 OpenJDK，LLD 使用 Windows LLVM 工具链而非保证固定 21 主版本；Crane 与 ngtcp2 库暂不自动安装。VS Code CLI 不可用，沿用旧扩展清单并保留警告，未宣称该来源采集完整。没有对本机执行工具升级或修改包管理器配置。

旧 `uipro-cli` 与 `ui-ux-pro-max-cli` 提供相同全局 `uipro` 命令，现仅自动安装后者，旧名称作为映射别名去重，避免 NPM shim 冲突。

## 最小设备验收

在可恢复的测试设备上，用目标账户运行；先选“开发工具”，不要首次试用就同时启用全部系统调优。

| 场景 | 应观察到的结果 |
| --- | --- |
| 启动、UAC、窗口与计划预览 | GUI 可用；工具与需管理员权限的操作清晰可见；预览不修改系统 |
| 本地清单导入 | 有效清单可用；无效、超大或未映射清单报错，原清单不变 |
| 受限网络与取消 | 默认来源可按条件回退；全部失败时明确报错；取消后退出运行态，可重新操作 |
| 首次安装与重跑 | 所选提供程序可用，软件确实可执行；重跑不伪造已安装/成功状态 |
| Scoop 特殊前提 | 新构建中，已有 Git 时不依赖 WinGet 安装 Git；自定义根目录安装后可立即查找 shim |
| 单项调优与撤销 | 写入前记录原值；日志符合真实状态；撤销后核对原值恢复 |

记录 Windows 版本/构建、账户方式、EXE 摘要、失败阶段和脱敏日志。没有这些设备证据时，只能称为已通过对应检查的试用构建。
