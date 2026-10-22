//! Storage policy inspection shares the cancellable worker; optimization uses deployment history.
use super::*;

impl Studio {
    pub(super) fn start_storage_status(&mut self) {
        if self.worker.is_some() {
            return;
        }
        self.storage_status = None;
        self.start_worker("正在读取存储通知策略", |cancel| {
            Ok(WorkerResult::StorageStatus(
                studio_deployment::storage_status(cancel).map_err(|error| format!("{error:#}")),
            ))
        });
    }

    pub(super) fn storage_panel(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(palette().surface)
            .stroke(Stroke::new(1.0, palette().line))
            .corner_radius(8)
            .inner_margin(14.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(tx("删除通知策略", "Delete-notification policy")).strong());
                    if ui.add_enabled(
                        self.worker.is_none(),
                        egui::Button::new(tx("只读检查 NTFS / ReFS", "Inspect NTFS / ReFS")),
                    ).clicked() {
                        self.start_storage_status();
                    }
                });
                ui.label(RichText::new(tx(
                    "检查只读取 Windows 策略，不启动磁盘分析或优化，也不修改 TRIM 设置。",
                    "Inspection reads Windows policy only. It does not analyze or optimize disks or change TRIM settings.",
                )).size(12.0).color(palette().muted));
                match &self.storage_status {
                    None => { ui.label(tx("尚未检查；不会在打开页面时自动读取。", "Not inspected. Opening this page does not query the system.")); }
                    Some(Err(error)) => {
                        ui.colored_label(palette().danger, tx("检查失败，不能确定当前通知策略。", "Inspection failed; the current policy is unknown."));
                        ui.label(error);
                    }
                    Some(Ok(status)) if status.preview => {
                        ui.label(tx("预览模式 · 未读取 Windows 的 NTFS / ReFS 状态。", "Preview · Windows NTFS / ReFS state was not read."));
                    }
                    Some(Ok(status)) => {
                        for (filesystem, state) in [("NTFS", status.ntfs), ("ReFS", status.refs)] {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(filesystem).strong());
                                ui.label(notification_label(state));
                            });
                        }
                        ui.label(RichText::new(tx(
                            "开启仅表示允许发送删除通知；不证明设备已执行 TRIM，也不代表磁盘健康或性能。",
                            "Enabled means delete notifications are allowed. It does not prove device TRIM execution, disk health or performance.",
                        )).size(12.0).color(palette().muted));
                    }
                }
                if let Some(Ok(status)) = &self.storage_status {
                    egui::CollapsingHeader::new(tx("查看原始检查说明", "Inspection details"))
                        .id_salt("storage-policy-details")
                        .show(ui, |ui| { ui.label(&status.detail); });
                }
            });
        ui.add_space(10.0);
        ui.label(RichText::new(tx(
            "所有卷包含 Windows 命令可处理的全部卷。优化没有撤销功能；取消后，Windows 存储服务可能仍在完成工作。",
            "All volumes includes every volume handled by the Windows command. Optimization has no undo; Windows storage services may continue after cancellation.",
        )).size(12.0).color(palette().muted));
        ui.hyperlink_to(tx("微软磁盘优化说明", "Microsoft disk optimization documentation"),
            "https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/defrag");
        ui.add_space(12.0);
    }
}

pub(super) fn task_command(id: &str) -> Option<String> {
    let (scope, operation) = match id {
        "storage-analyze-system" => (tx("<Windows 系统卷 GUID>", "<Windows volume GUID>"), "/A"),
        "storage-optimize-system" => (tx("<Windows 系统卷 GUID>", "<Windows volume GUID>"), "/O"),
        "storage-analyze-all" => ("/C", "/A"),
        "storage-optimize-all" => ("/C", "/O"),
        _ => return None,
    };
    Some(format!("defrag.exe {scope} {operation} /U"))
}

fn notification_label(state: studio_deployment::NotificationState) -> &'static str {
    use studio_deployment::NotificationState::*;
    match state {
        Enabled => tx("删除通知已开启", "Delete notifications enabled"),
        Disabled => tx("删除通知已关闭", "Delete notifications disabled"),
        NotSet => tx(
            "未显式设置，不能据此判断硬件行为",
            "Not explicitly set; hardware behavior is not established",
        ),
        Unknown => tx(
            "无法识别；未推定为开启",
            "Unrecognized; not assumed enabled",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_labels_keep_enabled_disabled_unset_and_unknown_distinct() {
        use studio_deployment::NotificationState::*;
        for language in [Language::ZhCn, Language::EnUs] {
            i18n::set_language(language);
            let labels: BTreeSet<_> = [Enabled, Disabled, NotSet, Unknown]
                .into_iter()
                .map(notification_label)
                .collect();
            assert_eq!(labels.len(), 4);
        }
        i18n::set_language(Language::ZhCn);
    }
}
