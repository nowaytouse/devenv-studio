//! Reviewed cross-platform toolchain requests; runtime installation stays in the Rust backend.
use super::*;
use studio_deployment::toolchains::{self as native, RustRequest};

pub(super) struct RustUiState {
    pub channel: String,
    pub confirmed: bool,
    pub installed: Option<Vec<String>>,
    pub report: Option<native::RustReport>,
}
impl Default for RustUiState {
    fn default() -> Self {
        Self {
            channel: "stable".into(),
            confirmed: false,
            installed: None,
            report: None,
        }
    }
}
impl Studio {
    pub(super) fn rust_file_operation(
        &mut self,
        operation: FileOperation,
        path: &std::path::Path,
    ) -> Result<String, String> {
        let request = RustRequest {
            channel: self.backup.rust.channel.clone(),
        };
        let result = match operation {
            FileOperation::SaveRustRequest => native::save(&request, path),
            FileOperation::LoadRustRequest => native::load(path).map(|request| {
                self.backup.rust.channel = request.channel;
                self.backup.rust.confirmed = false;
            }),
            FileOperation::ExportRustToolchain => native::rust_toolchain_toml(&request)
                .and_then(|text| studio_core::export_bytes(text.as_bytes(), path)),
            FileOperation::ExportMise => native::mise_toml(&request)
                .and_then(|text| studio_core::export_bytes(text.as_bytes(), path)),
            FileOperation::SaveRustReport => self
                .backup
                .rust
                .report
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("No Rust redeployment result"))
                .and_then(|report| studio_core::export_document(report, path)),
            _ => Err(anyhow::anyhow!("Not a Rust toolchain operation")),
        };
        result.map_err(|error| format!("{error:#}"))?;
        Ok(lf!(
            "已完成文件操作：{}",
            "File operation completed: {}",
            path.display()
        ))
    }
    pub(super) fn start_rust_redeploy(&mut self) {
        if !cfg!(any(target_os = "macos", windows))
            || self.worker.is_some()
            || !self.backup.rust.confirmed
        {
            return;
        }
        let request = RustRequest {
            channel: self.backup.rust.channel.clone(),
        };
        if let Err(error) = native::validate(&request) {
            self.notice = Some(Notice {
                text: error.to_string(),
                error: true,
            });
            return;
        }
        self.backup.rust.confirmed = false;
        self.backup.rust.report = None;
        self.start_network_worker("正在重部署 Rust 工具链", move |cancel| {
            native::configure(request, cancel).map(WorkerResult::Rust)
        });
    }
    pub(super) fn rust_toolchains_page(&mut self, ui: &mut egui::Ui) {
        ui.label(tx(
            "选择稳定版、预览渠道或固定版本，审核后重部署。",
            "Choose a stable, preview or exact toolchain, then review before redeployment.",
        ));
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    cfg!(any(target_os = "macos", windows)) && self.worker.is_none(),
                    egui::Button::new(tx(
                        "读取已安装 Rust 工具链",
                        "Read installed Rust toolchains",
                    )),
                )
                .clicked()
            {
                self.start_worker("正在读取 Rust 工具链", |cancel| {
                    native::capture(cancel).map(WorkerResult::RustInventory)
                });
            }
            if ui
                .button(tx("载入重部署请求…", "Load redeployment request…"))
                .clicked()
            {
                self.open_file(FileOperation::LoadRustRequest);
            }
        });
        if let Some(installed) = &self.backup.rust.installed {
            let mut choose = None;
            ui.horizontal_wrapped(|ui| {
                ui.label(lf!(
                    "本次读取 {} 项：",
                    "Observed {} toolchains: ",
                    installed.len()
                ));
                for channel in installed {
                    if ui.small_button(channel).clicked() {
                        choose = Some(channel.clone());
                    }
                }
            });
            if let Some(channel) = choose {
                self.backup.rust.channel = channel;
                self.backup.rust.confirmed = false;
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(tx("渠道 / 固定版本", "Channel / exact version"));
            for channel in ["stable", "beta", "nightly"] {
                if ui
                    .selectable_label(self.backup.rust.channel == channel, channel)
                    .clicked()
                {
                    self.backup.rust.channel = channel.into();
                    self.backup.rust.confirmed = false;
                }
            }
        });
        if ui
            .add(
                egui::TextEdit::singleline(&mut self.backup.rust.channel).hint_text(tx(
                    "例如 1.95.0 或 nightly-2026-10-20",
                    "For example 1.95.0 or nightly-2026-10-20",
                )),
            )
            .changed()
        {
            self.backup.rust.confirmed = false;
        }
        let request = RustRequest {
            channel: self.backup.rust.channel.clone(),
        };
        let validation = native::validate(&request);
        if let Err(error) = &validation {
            ui.colored_label(palette().danger, error.to_string());
        }
        if let Ok(text) = native::rust_toolchain_toml(&request) {
            egui::CollapsingHeader::new(tx(
                "审核原生重部署清单",
                "Review native redeployment manifests",
            ))
            .show(ui, |ui| {
                ui.label("rust-toolchain.toml");
                ui.monospace(text);
                ui.label("mise.toml");
                if let Ok(text) = native::mise_toml(&request) {
                    ui.monospace(text);
                }
            });
        }
        ui.horizontal_wrapped(|ui| {
            for (operation, label) in [
                (
                    FileOperation::SaveRustRequest,
                    tx("保存重部署请求…", "Save redeployment request…"),
                ),
                (
                    FileOperation::ExportRustToolchain,
                    tx("导出 rust-toolchain.toml…", "Export rust-toolchain.toml…"),
                ),
                (
                    FileOperation::ExportMise,
                    tx("导出 mise.toml…", "Export mise.toml…"),
                ),
            ] {
                if ui
                    .add_enabled(validation.is_ok(), egui::Button::new(label))
                    .clicked()
                {
                    self.open_file(operation);
                }
            }
        });
        egui::CollapsingHeader::new(tx("Rust 渠道与部署范围", "Rust channels and deployment scope")).show(ui, |ui| {
            ui.label(tx("固定版本与按日 nightly 可保持请求不变；stable、beta、nightly 在执行时解析为该渠道当前版本。", "Exact releases and dated nightly retain fixed requests; stable, beta and nightly resolve their current channel version when applied."));
            ui.label(tx("使用 minimal 配置安装，不更改默认工具链或项目覆盖。执行前保存本机清单与请求，完成后核对工具链身份、编译器版本及提交标识。读取清单不包含自定义链接工具链、组件或额外编译目标。mise 目前接入原生清单导出，尚未执行 mise 安装或生成锁文件。", "Installs the minimal profile without changing the default or project overrides. Saves a local baseline and request before installation, then checks toolchain identity, compiler release and commit. Inventory excludes custom linked toolchains, components and extra compilation targets. mise integration exports its native manifest; installation and lock generation are not integrated."));
        });
        ui.label(RichText::new(tx("需要标准位置的 rustup 和已有默认工具链；下载与安装没有自动回滚。", "Requires rustup in a standard location with an existing default toolchain; downloads and installation have no automatic rollback.")).size(12.0).color(palette().muted));
        ui.checkbox(&mut self.backup.rust.confirmed, tx("已审核渠道与版本；允许下载并安装所选工具链，无自动回滚", "Reviewed channel/version; allow download and installation of this toolchain, without automatic rollback"));
        if primary(
            ui,
            tx(
                "在本机重部署 Rust 工具链",
                "Redeploy Rust toolchain on this machine",
            ),
            cfg!(any(target_os = "macos", windows))
                && self.worker.is_none()
                && self.backup.rust.confirmed
                && validation.is_ok(),
        )
        .clicked()
        {
            self.start_rust_redeploy();
        }
        if let Some(report) = &self.backup.rust.report {
            ui.label(lf!(
                "最近请求：{} · 已验证 {}",
                "Last request: {} · verified {}",
                report.requested,
                report.verified
            ));
            if let Some(release) = &report.resolved_release {
                ui.monospace(release);
            }
            if let Some(hash) = &report.commit_hash {
                ui.monospace(hash);
            }
            ui.label(&report.detail);
            if ui
                .button(tx(
                    "保存 Rust 重部署结果…",
                    "Save Rust redeployment result…",
                ))
                .clicked()
            {
                self.open_file(FileOperation::SaveRustReport);
            }
        }
    }
}
