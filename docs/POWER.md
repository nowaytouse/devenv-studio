# 电源与节能

0.6新增16个插电（AC）选择，覆盖五组参数：显示器5/15/30分钟或从不、睡眠15/30/60分钟或从不、磁盘空闲10/20分钟或从不、USB选择性挂起启用/禁用，以及PCIe链路节能关闭/适中/最大。所有选择默认关闭，同组只能选择一个值。显示器、睡眠和磁盘参数以秒写入；USB与PCIe使用明确的枚举值。

## 操作范围

这里只修改执行开始时捕获的当前活动电源方案，不创建、删除、复制或切换到另一方案，不修改电池（DC）参数、CPU状态、休眠开关或自适应休眠阈值。Windows权限或组织策略可能阻止读写，相关失败会明确报告。macOS/Linux仅整理、保存、导出方案，检查结果明确为预览。

PowerWriteACValueIndex写入后还要通过PowerSetActiveScheme重新应用同一方案；原生返回值及实际回查均作为结果依据。执行期间请避免在其他软件切换方案：程序会在读写与激活前后检查，但Windows没有提供跨进程的原子比较写入保证，外部软件若恰好在最终检查与激活调用之间切换，仍可能被重新应用原方案。数值回查一致只证明参数记录匹配，不保证特定硬件实际进入睡眠或关闭设备。Modern Standby等机制可能限制磁盘空闲策略的作用。关闭USB选择性挂起会增加耗电，微软建议保持开启；该关闭项属于用户明确选择的高级选项。

## 原值与恢复

执行前记录活动方案GUID、可信参数地址、实际原值与期望值，持久化后才修改。电源日志独立于注册表日志和部署历史，并共享同一变更锁。应用过程中发现活动方案或参数被外部修改时停止；取消会保留已执行部分与日志。

电源恢复只处理所选日志记录的方案与参数；活动方案已切换、方案被删除、当前值与记录不一致等情况会停止，避免覆盖后来修改或强行切回旧方案。请保留对应日志与应用数据目录中的recovery.key；只有日志本身不足以授权恢复。恢复不会删除方案，也不是整个系统的恢复点。

## 档案与迁移

新的v3工作区将注册表、部署任务和电源选择保存在同一文件。仍能读取v1/v2，旧档案的电源集合为空；旧档案不会凭空获得电源操作。导入只接收可信选择ID，不接受任意GUID、原生命令或自定义电源地址。参考[电源工作区示例](../examples/power-workspace.json)。

旧工具的activate_ultimate_performance/extreme_ac_power_settings会创建或切换卓越性能方案，并可能修改CPU与休眠参数，与本页的当前方案AC选择不同。因此旧档案保留人工处理说明，不自动做部分替代。尤其休眠超时0可能代表自适应阈值，不能将其泛称为永不休眠。

## 官方依据

- [PowerGetActiveScheme](https://learn.microsoft.com/en-us/windows/win32/api/powersetting/nf-powersetting-powergetactivescheme)
- [PowerReadACValueIndex](https://learn.microsoft.com/en-us/windows/win32/api/powrprof/nf-powrprof-powerreadacvalueindex)
- [PowerWriteACValueIndex](https://learn.microsoft.com/en-us/windows/win32/api/powersetting/nf-powersetting-powerwriteacvalueindex)
- [PowerSetActiveScheme](https://learn.microsoft.com/en-us/windows/win32/api/powersetting/nf-powersetting-powersetactivescheme)
- [USB selective suspend](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/usb-selective-suspend)
- [Disk idle timeout](https://learn.microsoft.com/en-us/windows-hardware/customize/power-settings/disk-settings-disk-idle-timeout)
- [IPM disk idle limitations](https://learn.microsoft.com/en-us/windows-hardware/drivers/storage/ipm-hard-disk-drive-idle-timeout)
- [Hibernate idle timeout](https://learn.microsoft.com/en-us/windows-hardware/customize/power-settings/sleep-settings-hibernate-idle-timeout)

这些是实现与来源边界，Windows LTSC实机应用、外部冲突、断电中断与恢复效果仍需单独验收。
