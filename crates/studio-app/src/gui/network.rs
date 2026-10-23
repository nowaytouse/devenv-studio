//! Reviewed, application-scoped network policy and portable native configuration exports.
use super::*;
use studio_deployment::network::{self as native, Format, NetworkProfile, ProxyMode};

pub(super) struct NetworkUiState {
    pub draft: NetworkProfile,
    pub active: Result<NetworkProfile, String>,
    pub confirmed: bool,
    pub last_history: Option<PathBuf>,
    pub format: Format,
}

impl Default for NetworkUiState {
    fn default() -> Self {
        Self {
            draft: NetworkProfile::default(),
            active: Ok(NetworkProfile::default()),
            confirmed: false,
            last_history: None,
            format: Format::Git,
        }
    }
}

impl NetworkUiState {
    pub(super) fn load() -> Self {
        let active = native::load_current().map_err(|error| format!("{error:#}"));
        Self {
            draft: active.as_ref().cloned().unwrap_or_default(),
            active,
            ..Self::default()
        }
    }
}

impl Studio {
    pub(super) fn network_file_operation(
        &mut self,
        operation: FileOperation,
        path: &std::path::Path,
    ) -> Result<String, String> {
        match operation {
            FileOperation::NetworkSave => native::save(&self.network.draft, path),
            FileOperation::NetworkLoad => native::load(path).map(|profile| {
                self.network.draft = profile;
                self.network.confirmed = false;
            }),
            FileOperation::NetworkExportConfig => {
                native::native_config(&self.network.draft, self.network.format)
                    .and_then(|text| studio_core::export_bytes(text.as_bytes(), path))
            }
            _ => Err(anyhow::anyhow!("Not a network profile operation")),
        }
        .map_err(|error| format!("{error:#}"))?;
        Ok(lf!(
            "已完成网络档案文件操作：{}",
            "Network profile file operation completed: {}",
            path.display()
        ))
    }

    fn activate_network_profile(&mut self) {
        if self.worker.is_some() || !self.network.confirmed {
            return;
        }
        self.network.confirmed = false;
        if let Err(error) = native::validate(&self.network.draft) {
            self.notice = Some(Notice {
                text: error.to_string(),
                error: true,
            });
            self.last_operation = self.notice.clone();
            return;
        }
        let result = native::apply_current(&self.network.draft).and_then(|history| {
            let active = native::load_current()?;
            anyhow::ensure!(
                active == self.network.draft,
                "Network profile readback differs from the reviewed draft"
            );
            self.network.active = Ok(active);
            self.network.last_history = Some(history.clone());
            Ok(history)
        });
        self.notice = Some(match result {
            Ok(history) => Notice {
                text: lf!(
                    "已保存并回读核对网络档案；下一次由本程序启动的适用任务将使用它。历史：{}",
                    "Network profile saved and checked by readback. The next supported task started by this app will use it. History: {}",
                    history.display()
                ),
                error: false,
            },
            Err(error) => {
                let text = lf!(
                    "网络档案保存或核对未完成。请审核文件与历史记录，再重新读取已启用档案：{error:#}",
                    "Network profile save or verification did not complete. Review the file and history, then reload the active profile: {error:#}",
                    error = error
                );
                self.network.active = Err(text.clone());
                Notice { text, error: true }
            }
        });
        self.last_operation = self.notice.clone();
    }

    pub(super) fn network_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(tx("开发网络环境", "Developer network environment"));
        ui.label(tx(
            "为本程序的开发工具任务保存代理与软件源，并导出可复用的原生配置。修改草稿后，审核并启用才会用于后续任务。",
            "Save proxy and package sources for developer tasks started by this app, and export reusable native configuration. Review and activate draft changes before subsequent tasks use them.",
        ));
        ui.add_space(10.0);
        match &self.network.active {
            Ok(active) => {
                ui.horizontal_wrapped(|ui| {
                    badge(ui, tx("已启用", "Active"), palette().wash, palette().accent);
                    ui.label(proxy_label(active.mode));
                    if active != &self.network.draft {
                        ui.label(
                            RichText::new(tx("草稿有待启用的修改", "Draft has pending changes"))
                                .color(palette().accent),
                        );
                    }
                });
            }
            Err(error) => {
                ui.colored_label(
                    palette().danger,
                    tx(
                        "网络档案尚未确认；适用任务将停止。",
                        "Network profile is not verified; supported tasks will stop.",
                    ),
                );
                ui.label(error);
            }
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button(tx("载入档案…", "Load profile…")).clicked() {
                self.open_file(FileOperation::NetworkLoad);
            }
            if ui
                .add_enabled(
                    self.worker.is_none(),
                    egui::Button::new(tx("重新读取已启用档案", "Reload active profile")),
                )
                .clicked()
            {
                self.network.active = native::load_current().map_err(|error| format!("{error:#}"));
                self.network.confirmed = false;
            }
            if ui
                .button(tx("将草稿改为已启用内容", "Reset draft to active"))
                .clicked()
                && let Ok(active) = &self.network.active
            {
                self.network.draft = active.clone();
                self.network.confirmed = false;
            }
        });
        ui.separator();
        let before = self.network.draft.clone();
        ui.heading(tx("连接方式", "Connection"));
        ui.horizontal_wrapped(|ui| {
            for mode in [ProxyMode::Inherit, ProxyMode::Direct, ProxyMode::Manual] {
                ui.selectable_value(&mut self.network.draft.mode, mode, proxy_label(mode));
            }
        });
        if self.network.draft.mode != before.mode && self.network.draft.mode != ProxyMode::Manual {
            self.network.draft.proxy_url.clear();
        }
        match self.network.draft.mode {
            ProxyMode::Inherit => {
                ui.label(tx("沿用启动本程序时的代理环境；它可能受外部工具配置影响。", "Uses the proxy environment inherited when this app started; external tool configuration may affect it."));
            }
            ProxyMode::Direct => {
                ui.label(tx(
                    "适用 HTTP(S) 工具清除代理变量，并要求绕过代理。",
                    "Supported HTTP(S) tools clear proxy variables and request proxy bypass.",
                ));
            }
            ProxyMode::Manual => {
                ui.label(tx(
                    "HTTP 代理地址，不含账号或密码",
                    "HTTP proxy address, without credentials",
                ));
                ui.add(
                    egui::TextEdit::singleline(&mut self.network.draft.proxy_url)
                        .hint_text("http://127.0.0.1:7890")
                        .desired_width(f32::INFINITY),
                );
            }
        }
        ui.add_space(10.0);
        ui.heading(tx("软件源", "Package sources"));
        ui.label(RichText::new(tx("源地址需要 HTTPS；Cargo 稀疏索引使用 sparse+https://…/。自定义源可提供不同的软件内容，请审核服务方。", "Sources require HTTPS; Cargo sparse indexes use sparse+https://…/. Custom sources can serve different packages; review the operator.")).size(12.0).color(palette().muted));
        network_field(ui, "npm registry", &mut self.network.draft.npm_registry);
        network_field(
            ui,
            tx(
                "Python 默认索引 · Pip / uv",
                "Python default index · Pip / uv",
            ),
            &mut self.network.draft.python_index,
        );
        network_field(
            ui,
            tx("Cargo crates.io 索引", "Cargo crates.io index"),
            &mut self.network.draft.cargo_index,
        );
        if ui
            .button(tx(
                "草稿恢复官方软件源",
                "Restore official sources in draft",
            ))
            .clicked()
        {
            let official = NetworkProfile::default();
            self.network.draft.npm_registry = official.npm_registry;
            self.network.draft.python_index = official.python_index;
            self.network.draft.cargo_index = official.cargo_index;
        }
        if self.network.draft != before {
            self.network.confirmed = false;
        }
        let validation = native::validate(&self.network.draft);
        if let Err(error) = &validation {
            ui.colored_label(palette().danger, error.to_string());
        }
        ui.add_space(10.0);
        egui::CollapsingHeader::new(tx("审核已启用内容与草稿", "Review active profile and draft"))
            .default_open(true)
            .show(ui, |ui| {
                if let Ok(active) = &self.network.active {
                    egui::Grid::new("network-review").num_columns(3).max_col_width((ui.available_width() / 3.0 - 12.0).max(100.0)).striped(true).show(ui, |ui| {
                        for title in [tx("项目", "Field"), tx("已启用", "Active"), tx("草稿", "Draft")] {
                            ui.label(RichText::new(title).strong());
                        }
                        ui.end_row();
                        for (name, active, draft) in [
                            (tx("代理方式", "Proxy mode"), proxy_label(active.mode), proxy_label(self.network.draft.mode)),
                            ("HTTP proxy", active.proxy_url.as_str(), self.network.draft.proxy_url.as_str()),
                            ("npm registry", active.npm_registry.as_str(), self.network.draft.npm_registry.as_str()),
                            ("Python index", active.python_index.as_str(), self.network.draft.python_index.as_str()),
                            ("Cargo index", active.cargo_index.as_str(), self.network.draft.cargo_index.as_str()),
                        ] {
                            ui.label(name);
                            ui.label(active);
                            ui.label(RichText::new(draft).color(if active == draft { palette().muted } else { palette().accent }));
                            ui.end_row();
                        }
                    });
                } else {
                    ui.label(tx("请先重新读取档案。如文件损坏，先修复文件或将原文件保留到其他位置，再启用有效草稿；本程序不会覆盖损坏档案。", "Reload the profile first. If the file is malformed, correct it or preserve it elsewhere before activating a valid draft; this app will not overwrite a malformed profile."));
                }
            });
        ui.checkbox(
            &mut self.network.confirmed,
            tx(
                "已审核代理、软件源及适用范围；保存并用于后续任务",
                "Reviewed proxy, sources and scope; save for subsequent tasks",
            ),
        );
        ui.horizontal_wrapped(|ui| {
            if primary(
                ui,
                tx("保存并启用档案", "Save and activate profile"),
                self.worker.is_none() && validation.is_ok() && self.network.confirmed,
            )
            .clicked()
            {
                self.activate_network_profile();
            }
            if ui
                .add_enabled(
                    validation.is_ok(),
                    egui::Button::new(tx("导出网络档案…", "Export network profile…")),
                )
                .clicked()
            {
                self.open_file(FileOperation::NetworkSave);
            }
        });
        if let Some(history) = &self.network.last_history {
            ui.label(lf!(
                "最近保存历史：{}",
                "Last save history: {}",
                history.display()
            ));
            ui.label(RichText::new(tx("可载入历史中的 before.json 作为草稿，再审核启用以恢复。", "Load before.json from history as a draft, then review and activate it to restore.")).size(12.0).color(palette().muted));
        }
        ui.separator();
        egui::CollapsingHeader::new(tx("原生配置导出", "Native configuration export"))
            .show(ui, |ui| {
                egui::ComboBox::from_label(tx("配置格式", "Configuration format"))
                    .selected_text(format_label(self.network.format))
                    .show_ui(ui, |ui| {
                        for format in [Format::Git, Format::Cargo, Format::Npm, Format::Pip, Format::Uv] {
                            ui.selectable_value(&mut self.network.format, format, format_label(format));
                        }
                    });
                match native::native_config(&self.network.draft, self.network.format) {
                    Ok(text) => {
                        egui::ScrollArea::both().id_salt("network-native-preview").max_height(180.0).show(ui, |ui| { ui.monospace(text); });
                        if ui.button(tx("导出为新文件…", "Export as a new file…")).clicked() {
                            self.open_file(FileOperation::NetworkExportConfig);
                        }
                    }
                    Err(error) => { ui.colored_label(palette().danger, error.to_string()); }
                }
                ui.label(RichText::new(tx("导出仅创建新文件，不覆盖全局配置。uv.toml 保存索引；代理仍通过进程环境使用。", "Export creates a new file without replacing global settings. uv.toml contains indexes; proxy settings still use the process environment.")).size(12.0).color(palette().muted));
            });
        ui.add_space(10.0);
        ui.label(RichText::new(tx("适用范围：本程序随后启动的 Homebrew、rustup、Cargo、npm、Pip、uv 的受支持 HTTP(S) 操作。WinGet、DISM、VS Code、SSH、系统代理、DNS、VPN 不受此页控制；这不提供网络隔离。", "Scope: supported HTTP(S) operations in Homebrew, rustup, Cargo, npm, Pip and uv subsequently started by this app. This page does not control WinGet, DISM, VS Code, SSH, system proxy, DNS or VPN, and does not provide network isolation.")).size(12.0).color(palette().muted));
    }
}

fn network_field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.label(label);
    ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
}

fn proxy_label(mode: ProxyMode) -> &'static str {
    match mode {
        ProxyMode::Inherit => tx("继承代理环境", "Inherit proxy environment"),
        ProxyMode::Direct => tx("直连", "Direct"),
        ProxyMode::Manual => tx("明确 HTTP 代理", "Explicit HTTP proxy"),
    }
}

fn format_label(format: Format) -> &'static str {
    match format {
        Format::Git => "Git · .gitconfig",
        Format::Cargo => "Cargo · config.toml",
        Format::Npm => "npm · .npmrc",
        Format::Pip => "Pip · pip.conf / pip.ini",
        Format::Uv => "uv · uv.toml",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ui_is_unconfirmed_and_has_no_persistence_side_effect() {
        let state = NetworkUiState::default();
        assert!(!state.confirmed);
        assert_eq!(state.active.as_ref().ok(), Some(&state.draft));
        assert!(state.last_history.is_none());
        assert!(native::validate(&state.draft).is_ok());
    }
}
