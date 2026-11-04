use super::*;
use maintenance::windows_updates::{self, TerminalCommands};
use windows_inventory::{Inventory, Package};

#[derive(Default)]
pub(super) struct View {
    inventory: Option<Inventory>,
    error: Option<String>,
    query: String,
    selected: Option<usize>,
}

impl View {
    pub(super) fn accept(&mut self, result: WindowsInventory) -> Notice {
        self.selected = None;
        match result {
            Ok(inventory) => {
                self.inventory = Some(inventory);
                self.error = None;
                Notice {
                    text: tx(
                        "已读取 WinGet 可识别的软件。",
                        "Loaded the packages recognized by WinGet.",
                    )
                    .into(),
                    error: false,
                }
            }
            Err(error) => {
                self.error = Some(error.clone());
                Notice {
                    text: error,
                    error: true,
                }
            }
        }
    }

    fn commands(&self, package: &Package) -> Option<TerminalCommands> {
        if self.error.is_some() {
            return None;
        }
        windows_updates::commands(package)
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, busy: bool) -> bool {
        ui.heading(tx("Windows 已安装软件", "Installed Windows software"));
        ui.label(tx(
            "清单来自 WinGet 导出，覆盖它能匹配到来源的软件。版本可能缺失，清单不包含可用更新状态。",
            "The WinGet export covers software it can match to a source. Versions may be missing, and the inventory does not report update availability.",
        ));
        let refresh = primary(
            ui,
            if self.inventory.is_some() {
                tx("刷新清单", "Refresh inventory")
            } else {
                tx("读取 WinGet 清单", "Read WinGet inventory")
            },
            !busy,
        )
        .clicked();
        if let Some(error) = &self.error {
            ui.colored_label(palette().danger, error);
            if self.inventory.is_some() {
                ui.label(tx(
                    "显示上一次成功读取的清单。刷新成功后可重新生成终端命令。",
                    "Showing the last successful inventory. Refresh successfully to use terminal commands again.",
                ));
            }
        }
        let Some(inventory) = &self.inventory else {
            return refresh;
        };
        ui.add_space(8.0);
        ui.label(lf!(
            "WinGet 导出 {} 项",
            "WinGet exported {} packages",
            inventory.packages.len()
        ));
        let changed = ui
            .add(egui::TextEdit::singleline(&mut self.query).hint_text(tx(
                "搜索包 ID、来源或版本",
                "Search package ID, source, or version",
            )))
            .changed();
        if changed {
            self.selected = None;
        }
        let query = self.query.trim().to_ascii_lowercase();
        let visible: Vec<_> = inventory
            .packages
            .iter()
            .enumerate()
            .filter(|(_, package)| {
                [
                    &*package.id,
                    &*package.source,
                    package.version.as_deref().unwrap_or_default(),
                ]
                .iter()
                .any(|value| value.to_ascii_lowercase().contains(&query))
            })
            .collect();
        if visible.is_empty() {
            ui.label(if inventory.packages.is_empty() {
                tx(
                    "WinGet 未导出匹配的软件。",
                    "WinGet exported no matched packages.",
                )
            } else {
                tx("没有符合条件的软件。", "No matching packages.")
            });
        } else {
            let height = ui.spacing().interact_size.y.max(28.0);
            let mut list = egui::ScrollArea::vertical()
                .id_salt("update-winget-packages")
                .max_height(280.0);
            if changed {
                list = list.vertical_scroll_offset(0.0);
            }
            list.show_rows(ui, height, visible.len(), |ui, range| {
                for &(index, package) in &visible[range] {
                    let selected = self.selected == Some(index);
                    let colors = palette();
                    let version = package
                        .version
                        .as_deref()
                        .unwrap_or_else(|| tx("版本未知", "Version unknown"));
                    let summary = format!("{} · {version}", package.source);
                    let mut button = egui::Button::selectable(
                        selected,
                        RichText::new(&package.id).color(colors.ink),
                    )
                    .right_text(RichText::new(summary).color(colors.muted))
                    .truncate();
                    if selected {
                        button = button.fill(colors.wash);
                    }
                    if ui
                        .add_sized([ui.available_width(), height], button)
                        .clicked()
                    {
                        self.selected = Some(index);
                    }
                }
            });
        }
        if let Some(package) = self
            .selected
            .and_then(|index| inventory.packages.get(index))
        {
            ui.add_space(8.0);
            surface_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.strong(&package.id);
                ui.label(lf!("来源 · {}", "Source · {}", package.source));
                ui.label(lf!(
                    "导出版本 · {}",
                    "Exported version · {}",
                    package.version.as_deref().unwrap_or_else(|| tx("未知", "Unknown"))
                ));
                if let Some(commands) = self.commands(package) {
                    ui.add_space(8.0);
                    ui.strong(tx("在终端核对和升级此软件", "Check and upgrade this package in Terminal"));
                    ui.label(tx(
                        "先运行核对命令，再决定是否升级。Studio 只复制命令，终端中的操作由你确认。",
                        "Run the check first, then decide whether to upgrade. Studio copies the commands for you to review and run in Terminal.",
                    ));
                    command_row(ui, &commands.check, tx("复制核对命令", "Copy check command"), busy);
                    command_row(ui, &commands.upgrade, tx("复制单包升级命令", "Copy package upgrade command"), busy);
                } else if self.error.is_none() {
                    ui.label(tx(
                        "只有来源和身份明确的官方 WinGet 社区包可生成单包命令。",
                        "Per-package commands are available for unambiguous packages from the official WinGet community source.",
                    ));
                }
            });
        }
        egui::CollapsingHeader::new(tx("读取命令示例", "Inventory command example")).show(
            ui,
            |ui| {
                ui.label(RichText::new(inventory.command).monospace());
            },
        );
        refresh
    }
}

fn command_row(ui: &mut egui::Ui, command: &str, label: &str, busy: bool) {
    ui.label(RichText::new(command).monospace());
    if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
        ui.ctx().copy_text(command.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_refresh_retains_inventory_but_blocks_old_commands() {
        let package = Package {
            id: "Git.Git".into(),
            source: "winget".into(),
            version: Some("2.50.0".into()),
            official_winget: true,
            can_target: true,
        };
        let inventory = Inventory {
            packages: vec![package],
            command: "winget export",
        };
        let mut view = View::default();
        assert!(!view.accept(Ok(inventory)).error);
        assert!(
            view.commands(&view.inventory.as_ref().unwrap().packages[0])
                .is_some()
        );
        assert!(view.accept(Err("failed refresh".into())).error);
        assert!(
            view.commands(&view.inventory.as_ref().unwrap().packages[0])
                .is_none()
        );
    }
}
