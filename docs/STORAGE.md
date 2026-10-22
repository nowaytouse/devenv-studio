# 存储检查与优化

0.7提供独立的存储页面。NTFS/ReFS删除通知是只读状态检查，磁盘分析与优化是明确选择的维护任务，两者互不隐含启用。

## 四项维护选择

| ID | 范围 | 原生命令参数 |
| --- | --- | --- |
| storage-analyze-system | Windows系统所在卷 | <系统卷GUID> /A /U |
| storage-optimize-system | Windows系统所在卷 | <系统卷GUID> /O /U |
| storage-analyze-all | Windows命令可处理的所有卷 | /C /A /U |
| storage-optimize-all | Windows命令可处理的所有卷 | /C /O /U |

原生命令为Windows自身目录下的System32/defrag.exe，不走PATH、shell或下载脚本。这里的系统卷指Windows目录所在卷，不是EFI系统分区。系统卷通过GetSystemWindowsDirectoryW、GetVolumePathNameW和GetVolumeNameForVolumeMountPointW解析，严格验证卷GUID路径，不猜测C盘。实际目标、参数、命令输出及结果进入同一部署历史。GUID路径的真实LTSC运行效果仍需设备验收。

/C表示所有卷，不能标为“所有固定卷”。/O由Windows按介质选择适当优化，不能保证SSD只会retrim或绝不进行传统碎片整理。默认低优先级，不加/H或/M；单条defrag最多45分钟，整个执行会话仍受90分钟上限约束。分析和优化都需要已提升的管理员进程；应用不自行提权。

查看任务状态仅返回尚未执行的维护项目，不会自动分析或优化。执行前写入意图，完整捕获输出；只有退出0才作为命令完成，超时、取消、启动/捕获失败或其他退出码不能算完成。完成不等于每个卷都优化、磁盘健康或性能提升，具体限制与跳过信息需查看Windows原始报告。磁盘优化不属于注册表或电源恢复范围，不提供撤销；取消会终止直接命令，但Windows存储服务仍可能继续。

## 删除通知策略

独立按钮与命令行storage-status只调用fsutil.exe behavior query DisableDeleteNotify，不执行set、分析或优化。

- 按NTFS/ReFS分别解析稳定字段，0表示删除通知开启，1表示关闭。
- 官方英文“is not currently set”保留为未设；其他本地化或未识别内容保留未知，缺失项也为未知。
- 重复文件系统字段、非法数值或混杂数值报错；非零退出、取消、超时与输出不完整不能变成正常状态。
- 允许发送删除通知不证明设备支持或完成TRIM/UNMAP，也不检查磁盘健康。
- macOS/Linux返回明确预览和未知状态，不读取本机磁盘模拟Windows结果。Windows需要受支持的LTSC主机和管理员权限。

旧工具将查询输出折叠为一个数字，再设置通知策略，不能准确保留独立NTFS/ReFS及未设状态。当前只迁移其存储优化请求，通知修改与恢复保留手动说明；不复制这种恢复方式。

## 来源

- [Microsoft defrag：范围、分析、按介质优化、权限与SSD说明](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/defrag)
- [Microsoft fsutil behavior：删除通知、0/1及未设示例](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/fsutil-behavior)
- [Windows卷GUID路径](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-volume)
- [GetSystemWindowsDirectoryW](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-getsystemwindowsdirectoryw)
- [旧工具存储流程固定来源](https://github.com/nowaytouse/LTSC_Tools_Backup/blob/a8523abbe08147b4d5de8746f26d99d056057ab0/src/installer.rs#L1551)
