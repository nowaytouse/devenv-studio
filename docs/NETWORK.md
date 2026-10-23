# 开发环境网络配置（0.10）

网络配置作用于 DevEnv Studio 图形界面随后启动的开发环境部署任务。每次任务开始时固定本次配置，后台执行期间不能切换；修改后需启用才会供下一次任务使用。不会修改系统代理、DNS、VPN、Shell 启动文件或用户现有包管理器配置，也不会联网探测代理或镜像可用性。

## 代理与软件源

| 选择 | 含义 |
| --- | --- |
| 继承当前环境 | 保留本程序启动时继承的通用 HTTP(S) 代理环境；继续保留提供程序原有的隔离规则，不恢复被安全清理的任意配置。GUI 启动环境可能与终端不同。 |
| 直连 | 为受支持的部署子进程清理通用代理及冲突的提供程序代理配置，并明确禁用本次 HTTP(S) 代理。系统或应用之外的网络转发不在此承诺内。 |
| 手动 HTTP 代理 | 使用明确输入的 HTTP 代理，统一配置 HTTP、HTTPS 请求的代理环境，覆盖旧通用代理。HTTPS 目标通过 HTTP CONNECT 代理访问，不因此关闭目标证书验证。 |

手动代理仅接受不带认证信息的 HTTP 地址。用户名、密码、令牌、查询参数、片段与可注入配置行的控制字符不属于可保存内容。此版本不提供 PAC、SOCKS、代理认证或绕过证书检查的选项。

npm、Python 和 Cargo 可分别选择 HTTPS 软件源；初始值使用已有官方来源，地址不能留空。Python 设置同时用于 Pip 与 uv 的默认包索引。Cargo 输入采用以斜线结尾的 `sparse+https://…/` 原生索引格式，自定义镜像运行及导出时以 source replacement 替换逻辑 `crates-io` 来源；官方默认值保留 Cargo 现有配置行为，已有本地 source replacement 仍可能重定向实际来源。它应是 crates.io 的等价镜像，不能当成任意私有注册表或包内容补丁功能。

自定义源是用户明确选择的下载来源，会改变索引、元数据和软件包的交付路径。HTTPS 与包管理器既有校验不等于对镜像运营方或内容作出信任保证；配置未包含凭据，也不把网络地址可解析当成重部署成功。Rustup 与 Homebrew 本轮只接入代理，不提供自定义下载镜像。

## 真正接入的执行范围

- macOS：GUI 中的 Homebrew 补齐、滚动与 HEAD 部署，Rustup 渠道、mise core、uv 工具，以及审核过的 Cargo CLI 重部署。
- Windows：GUI 中的 Cargo、npm、Pip、uv 开发工具部署、Rustup 渠道与 mise core 重部署。
- 各提供程序继续保留既有的固定可执行文件定位、安装范围、参数校验、结果回读与隔离要求；npm 仍禁止生命周期脚本，Pip 仍要求受支持解释器、用户范围与 wheel 包，uv 仍不自动下载 Python。

读取备份和本机清单、CLI 命令不启用这份 GUI 部署配置。WinGet、DISM、VS Code 扩展、SSH、系统网络设置及其他应用不列为受支持执行范围。Homebrew 公式和 Cargo 软件包的构建步骤可能访问额外网络；环境配置是提供给受支持工具的 HTTP(S) 路由设置，不能充当网络沙箱或阻止任意子程序自行联网。macOS Cargo 的 Git HEAD 查询仅访问目录中固定的 GitHub HTTPS 仓库，Cargo registry 镜像只影响 crates.io 依赖源，不会改写 Git URL。

提供程序差异由适配器明确处理：npm 的固定 registry 参数会替换为审核后的值；Pip 的隔离模式继续有效，因此通过命令行传入索引和代理；uv 使用明确默认索引；Cargo registry 安装通过 `--config` source replacement 应用 crates.io 镜像，Git HEAD 使用同一代理方案并保留固定 HTTPS 仓库地址。网络值在提供程序安全清理之后应用，避免界面已启用而子进程实际丢失配置。

## 保存、导入和执行记录

可以将配置保存为严格验证的 JSON，重新载入后审核并启用。载入配置与启用配置是两个动作，不能因导入文件而自动影响下一次部署。启用时保存应用私有配置、保留原值并回读；保存或回读失败不会显示启用成功。历史包含可重新载入的 `before.json`，已有文件另保留精确 `before.raw` 与是否原本存在的标记。可将 `before.json` 载入为草稿、审核后启用以恢复之前的值。若已有配置损坏，应用停止读取或替换，需先由用户修正或移走损坏文件，不能静默降为默认设置。部署期间不允许修改正在使用的配置。

Homebrew、Rustup 与 Windows 部署记录中保留本次固定配置的 `network.json`，供结果审查时核对实际请求。未处于应用网络作用域的调用只记录“未应用网络方案”，不凭空记录默认软件源为生效配置。网络配置不包含认证信息；自定义主机名、代理地址仍可能是个人或组织信息，分享清单前需审核。既有的请求记录、安装前基线、取消、失败和结果核对仍分别保留；网络记录写入失败会阻止后续部署，不能以默认配置继续并显示成功。

## 原生配置导出

导出创建新文件，不直接合并或覆盖用户配置。应先查看内容，再按对应工具规则放入需要的作用域；导出不会自动改变终端或系统行为。

| 格式 | 可表达的内容与边界 |
| --- | --- |
| Git 配置 | HTTP 代理配置片段。Git 自己的 URL 或 remote 配置可以更具体；不能通过这个片段完整覆盖 SSH 或其他网络策略。 |
| Cargo `config.toml` | HTTP 代理与 Cargo sparse source replacement；原有配置合并结果应由使用者审核。 |
| npm `.npmrc` | registry 与代理；直连的 `proxy=null`、`https-proxy=null` 不阻止 npm 继续读取外部通用代理环境，因此独立使用导出文件时还需清理终端的代理变量。 |
| Pip 配置 | `[global]` 索引与代理。没有 `trusted-host` 或关闭证书验证的设置；应用自身的 `--isolated` 部署仍使用审核后的参数，不读取导出文件。 |
| uv `uv.toml` | 默认 Python 索引；uv 的通用代理通过环境变量表达，不能伪装成此文件中不存在的原生 proxy 键。 |

配置导出表达所选设置，不保证不同工具对外部配置、继承环境或特定域名规则的优先级完全相同。尤其不能把 npm 的 `noproxy=*` 当成跨版本通用的直连保证；应用运行时显式清理冲突代理变量。

## 官方契约

本轮复用成熟工具的原生行为，不执行下载脚本，也不引入第三方脚本合并执行：

- [Homebrew 代理说明](https://docs.brew.sh/Manpage#using-homebrew-behind-a-proxy)与 [curl 代理环境规则](https://github.com/curl/everything-curl/blob/master/usingcurl/proxies/env.md)：HTTP、HTTPS 代理及大小写差异。
- [Cargo 配置](https://doc.rust-lang.org/cargo/reference/config.html#httpproxy)、[source replacement](https://doc.rust-lang.org/cargo/reference/source-replacement.html)与 [cargo install 配置发现](https://doc.rust-lang.org/cargo/commands/cargo-install.html#configuration-discovery)：运行参数、用户配置和逻辑软件源替换。
- [npm 配置](https://docs.npmjs.com/cli/v11/using-npm/config)与 [npm 代理实现](https://github.com/npm/agent/blob/main/lib/proxy.js)：命令行配置、环境回退与 bypass 匹配的实际边界。
- [Pip 通用选项](https://pip.pypa.io/en/stable/cli/pip/)与 [配置](https://pip.pypa.io/en/stable/topics/configuration/)：隔离模式、代理及索引优先级。
- [uv 环境](https://docs.astral.sh/uv/configuration/environment/)与 [索引](https://docs.astral.sh/uv/concepts/indexes/)：HTTP(S) 代理和默认索引。
- [Rustup 环境变量](https://rust-lang.github.io/rustup/environment-variables.html)：工具链下载与自更新来源是不同的配置；本轮不修改它们。

本地参数和进程环境回归、目标平台编译、真实代理／镜像连通性和真实安装验收分别报告；前两者不能替代后两者。
