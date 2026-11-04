use super::*;
use brew_inventory::{Kind, Package};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    Formula,
    Cask,
}

impl Filter {
    fn includes(self, kind: &Kind) -> bool {
        matches!(
            (self, kind),
            (Self::All, _) | (Self::Formula, Kind::Formula) | (Self::Cask, Kind::Cask)
        )
    }
}

#[derive(Default)]
pub(super) struct View {
    inventory: Option<BrewInventory>,
    filter: Filter,
    query: String,
    selected: Option<usize>,
}

impl View {
    pub(super) fn accept(&mut self, inventory: BrewInventory) -> Notice {
        let notice = match &inventory {
            Ok(_) => Notice {
                text: tx(
                    "已读取 Homebrew 本机清单。",
                    "Local Homebrew inventory loaded.",
                )
                .into(),
                error: false,
            },
            Err(error) => Notice {
                text: error.clone(),
                error: true,
            },
        };
        self.inventory = Some(inventory);
        self.selected = None;
        notice
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, busy: bool) -> bool {
        ui.heading(tx("Homebrew 本机清单", "Local Homebrew inventory"));
        ui.label(tx(
            "查看已安装版本、链接和固定版本。读取清单不会更新 Homebrew 或检查上游版本。",
            "Inspect installed versions, links, and pins. Reading the inventory does not update Homebrew or check upstream versions.",
        ));
        let read_label = if matches!(self.inventory, Some(Ok(_))) {
            tx("刷新清单", "Refresh inventory")
        } else {
            tx("读取已安装软件", "Read installed packages")
        };
        let refresh = primary(ui, read_label, !busy).clicked();
        let inventory = match &self.inventory {
            None => return refresh,
            Some(Err(error)) => {
                ui.colored_label(palette().danger, error);
                return refresh;
            }
            Some(Ok(inventory)) => inventory,
        };

        let formulae = inventory
            .packages
            .iter()
            .filter(|row| matches!(row.kind, Kind::Formula))
            .count();
        let casks = inventory.packages.len() - formulae;
        let previous_filter = self.filter;
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut self.filter,
                Filter::All,
                lf!("全部 {}", "All {}", inventory.packages.len()),
            );
            ui.selectable_value(
                &mut self.filter,
                Filter::Formula,
                format!("Formula {formulae}"),
            );
            ui.selectable_value(&mut self.filter, Filter::Cask, format!("Cask {casks}"));
        });
        let filter_changed = ui
            .add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text(tx("搜索已安装软件", "Search installed packages")),
            )
            .changed()
            || self.filter != previous_filter;
        if filter_changed {
            self.selected = None;
        }
        let query = self.query.trim().to_ascii_lowercase();
        let visible: Vec<_> = inventory
            .packages
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                self.filter.includes(&row.kind) && row.name.to_ascii_lowercase().contains(&query)
            })
            .collect();
        if visible.is_empty() {
            ui.label(if inventory.packages.is_empty() {
                tx(
                    "没有已安装的 Formula 或 Cask。",
                    "No installed formulae or casks.",
                )
            } else {
                tx("没有符合条件的软件。", "No matching packages.")
            });
        } else {
            let height = ui.spacing().interact_size.y.max(28.0);
            let mut list = egui::ScrollArea::vertical()
                .id_salt("update-homebrew-packages")
                .max_height(280.0);
            if filter_changed {
                list = list.vertical_scroll_offset(0.0);
            }
            list.show_rows(ui, height, visible.len(), |ui, range| {
                for &(index, row) in &visible[range] {
                    let version = row
                        .opt_version
                        .as_deref()
                        .or(row.linked_version.as_deref())
                        .or(match row.versions.as_slice() {
                            [version] => Some(version.as_str()),
                            _ => None,
                        });
                    let kind = kind_label(&row.kind);
                    let summary = match version {
                        Some(version) => format!("{kind}  ·  {version}"),
                        None => kind.to_owned(),
                    };
                    let selected = self.selected == Some(index);
                    let colors = palette();
                    let mut button = egui::Button::selectable(
                        selected,
                        RichText::new(&row.name).color(colors.ink),
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
        if let Some(row) = self
            .selected
            .and_then(|index| inventory.packages.get(index))
        {
            ui.add_space(8.0);
            surface_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                show_package(ui, row);
            });
        }

        egui::CollapsingHeader::new(tx("本次读取命令", "Inventory command")).show(ui, |ui| {
            ui.label(RichText::new(inventory.command).monospace());
            if ui.button(tx("复制命令", "Copy command")).clicked() {
                ui.ctx().copy_text(inventory.command.to_owned());
            }
        });
        refresh
    }
}

fn kind_label(kind: &Kind) -> &'static str {
    match kind {
        Kind::Formula => "Formula",
        Kind::Cask => "Cask",
    }
}

fn show_package(ui: &mut egui::Ui, row: &Package) {
    ui.strong(&row.name);
    ui.label(kind_label(&row.kind));
    ui.label(lf!(
        "已安装版本：{}",
        "Installed versions: {}",
        row.versions.join(" · ")
    ));
    for (label, version) in [
        (tx("已链接版本", "Linked version"), &row.linked_version),
        (
            tx("opt 指向的版本", "Version selected by opt"),
            &row.opt_version,
        ),
        (tx("固定版本", "Pinned version"), &row.pinned_version),
    ] {
        if let Some(version) = version {
            ui.label(format!("{label} · {version}"));
        }
    }
}
