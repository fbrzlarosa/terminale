// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_about(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "About", "");

        card(ui, |ui| {
            ui.label(
                egui::RichText::new("terminale")
                    .heading()
                    .color(egui::Color32::from_rgb(220, 230, 255)),
            );
            ui.label(
                egui::RichText::new(env!("CARGO_PKG_VERSION"))
                    .small()
                    .color(egui::Color32::from_rgb(120, 130, 160)),
            );
            ui.add_space(8.0);
            ui.label("A native, cross-platform, GPU-accelerated terminal.");
            ui.add_space(6.0);
            ui.hyperlink_to(
                "https://stackbyte.dev/terminale",
                "https://stackbyte.dev/terminale",
            );
            ui.add_space(4.0);
            ui.hyperlink_to(
                "github.com/fbrzlarosa/terminale",
                "https://github.com/fbrzlarosa/terminale",
            );
        });

        // ── Updates ──
        ui.add_space(10.0);
        card(ui, |ui| {
            let pre = ui.min_rect();
            ui.label(
                egui::RichText::new("Updates")
                    .strong()
                    .color(egui::Color32::from_rgb(220, 230, 255)),
            );
            self.highlight_row(ui, ui.min_rect().union(pre), Section::About, "Updates");
            sublabel(
                ui,
                "terminale updates itself from GitHub releases. Downloads are verified \
                 (SHA-256) and the binary is replaced on disk without interrupting your \
                 session — the new version applies on the next launch (never a forced \
                 restart). Legacy system-wide installs (MSI under Program Files) hand off \
                 to Windows Installer in silent mode instead: no wizard, just the one \
                 unavoidable elevation prompt.",
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, "Check for updates on startup");
                let on = self.config.updates.check_on_startup;
                if toggle_switch(ui, on).clicked() {
                    self.config.updates.check_on_startup = !on;
                    self.dirty = true;
                }
            });
            ui.horizontal(|ui| {
                field_label(ui, "Download and stage automatically");
                let on = self.config.updates.auto_install;
                if toggle_switch(ui, on).clicked() {
                    self.config.updates.auto_install = !on;
                    self.dirty = true;
                }
            });
            sublabel(
                ui,
                "Off = only notify when a new version exists. On = silently download, verify \
                 and stage it (applies on next launch).",
            );
            ui.add_space(6.0);
            // Disabled while a check is already running so the user can't spawn
            // a pile of concurrent threads by clicking repeatedly.
            let checking = self.update_rx.is_some();
            let label = if checking {
                "Checking…"
            } else {
                "Check for updates now"
            };
            if ui
                .add_enabled(!checking, egui::Button::new(label))
                .clicked()
            {
                // Runs in the background so the UI never blocks; the result is
                // sent back over a channel and surfaced as a visible status
                // line by `build_ui` (in addition to being logged).
                let (tx, rx) = std::sync::mpsc::channel();
                self.update_rx = Some(rx);
                self.status = Some((StatusKind::Success, "Checking for updates…".to_owned()));
                std::thread::spawn(move || {
                    use crate::update::UpdateOutcome;
                    // interactive=true: a manual click may hand off to the
                    // platform installer (UI + elevation prompt are expected).
                    let result =
                        crate::update::download_and_apply(true).map_err(|e| format!("{e:#}"));
                    match &result {
                        Ok(UpdateOutcome::Staged(v)) => {
                            tracing::info!(version = %v, "update staged; restart to apply");
                        }
                        Ok(UpdateOutcome::SwitchRequired(v)) => {
                            tracing::info!(
                                version = %v,
                                "legacy install: update available via the one-time switch"
                            );
                        }
                        Ok(UpdateOutcome::InstallerRequired(_)) | Ok(UpdateOutcome::UpToDate) => {
                            tracing::info!("update check finished");
                        }
                        Err(e) => tracing::warn!(error = %e, "manual update failed"),
                    }
                    // Receiver may be gone if the window closed mid-check; ignore.
                    let _ = tx.send(result);
                });
            }
            sublabel(
                ui,
                "Runs in the background; restart terminale when it's done. For full control \
                 from a shell: `terminale --update`.",
            );

            // ── One-time migration off the legacy per-machine MSI ───────────
            // Shown EXCLUSIVELY when this process runs from a non-writable
            // Program Files tree (the pre-0.1.27 system-wide MSI). Per-user
            // MSI and PowerShell installs live under %LOCALAPPDATA% and never
            // see this — they already self-update silently.
            if crate::update::is_legacy_machine_install() {
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Switch to the self-updating install")
                        .strong()
                        .color(egui::Color32::from_rgb(255, 210, 130)),
                );
                sublabel(
                    ui,
                    "This terminale was installed system-wide (Program Files), so every \
                     update needs Windows Installer and an elevation prompt. Switch once to \
                     the per-user install: the latest version is downloaded and verified, \
                     installed under your user profile, and the old system-wide copy is \
                     removed (one last elevation prompt — the last ever). From then on \
                     updates apply silently in the background. Running sessions in this \
                     window will close; terminale restarts automatically.",
                );
                ui.add_space(4.0);
                let migrating = self.update_rx.is_some();
                let label = if migrating {
                    "Working…"
                } else {
                    "Switch now (restarts terminale)"
                };
                if ui
                    .add_enabled(!migrating, egui::Button::new(label))
                    .clicked()
                {
                    let (tx, rx) = std::sync::mpsc::channel();
                    self.update_rx = Some(rx);
                    self.status = Some((
                        StatusKind::Success,
                        "Downloading and switching to the per-user install…".to_owned(),
                    ));
                    std::thread::spawn(move || {
                        #[cfg(windows)]
                        match crate::update::migrate_to_user_install() {
                            Ok(new_exe) => {
                                tracing::info!(
                                    exe = %new_exe.display(),
                                    "migrated to the per-user install; exiting so the \
                                     per-machine uninstall can proceed"
                                );
                                // The new copy is already running and msiexec
                                // needs this exe released — leave immediately.
                                std::process::exit(0);
                            }
                            Err(e) => {
                                tracing::warn!(error = %format!("{e:#}"), "migration failed");
                                let _ = tx.send(Err(format!("{e:#}")));
                            }
                        }
                        #[cfg(not(windows))]
                        {
                            let _ = tx.send(Err("only applicable on Windows".to_owned()));
                        }
                    });
                }
            }
        });

        // ── Diagnostics (file logging) ──
        ui.add_space(10.0);
        card(ui, |ui| {
            let pre = ui.min_rect();
            ui.label(
                egui::RichText::new("Diagnostics")
                    .strong()
                    .color(egui::Color32::from_rgb(220, 230, 255)),
            );
            self.highlight_row(ui, ui.min_rect().union(pre), Section::About, "Diagnostics");
            sublabel(
                ui,
                "A rolling daily log file next to the config, so a freeze or crash leaves \
                 evidence even when terminale is launched without a console.",
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                field_label(ui, "Write log file");
                let on = self.config.logging.file_enabled;
                if toggle_switch(ui, on).clicked() {
                    self.config.logging.file_enabled = !on;
                    self.dirty = true;
                }
            });
            ui.horizontal(|ui| {
                field_label(ui, "File log level");
                egui::ComboBox::from_id_salt("log_file_level_combo")
                    .selected_text(self.config.logging.file_level.clone())
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for level in ["error", "warn", "info", "debug", "trace"] {
                            if ui
                                .selectable_label(self.config.logging.file_level == level, level)
                                .clicked()
                            {
                                self.config.logging.file_level = level.to_owned();
                                self.dirty = true;
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                field_label(ui, "Keep logs for");
                let r = ui.add(
                    egui::DragValue::new(&mut self.config.logging.retention_days)
                        .range(1..=365)
                        .suffix(" days"),
                );
                if r.changed() {
                    self.dirty = true;
                }
            });
            sublabel(
                ui,
                "Enable/level apply on the next launch; older files are pruned at startup.",
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                field_label(ui, "Freeze watchdog");
                let enabled = self.config.logging.slow_frame_warn_ms != 0;
                if toggle_switch(ui, enabled).clicked() {
                    // Off ⇄ on; restore a sane default when re-enabling.
                    self.config.logging.slow_frame_warn_ms = if enabled { 0 } else { 250 };
                    self.dirty = true;
                }
            });
            if self.config.logging.slow_frame_warn_ms != 0 {
                ui.horizontal(|ui| {
                    field_label(ui, "Warn above");
                    let r = ui.add(
                        egui::DragValue::new(&mut self.config.logging.slow_frame_warn_ms)
                            .range(16..=60_000)
                            .suffix(" ms"),
                    );
                    if r.changed() {
                        self.dirty = true;
                    }
                });
            }
            self.highlight_row(ui, ui.min_rect(), Section::About, "Freeze watchdog");
            sublabel(
                ui,
                "Logs a warning when a single frame stalls past this threshold — catches \
                 transient freezes (GPU reset, blocking UI call) that recover on their own. \
                 Applies live.",
            );
            ui.add_space(6.0);
            if ui.button("Open logs folder").clicked() {
                if let Some(parent) = self.config_path.parent() {
                    let _ = open::that_detached(parent.join("logs"));
                }
            }
        });

        ui.add_space(10.0);
        card(ui, |ui| {
            let pre_rect = ui.min_rect();
            ui.label(
                egui::RichText::new("Config file")
                    .strong()
                    .color(egui::Color32::from_rgb(220, 230, 255)),
            );
            let label_rect = ui.min_rect().union(pre_rect);
            self.highlight_row(ui, label_rect, Section::About, "Config file");
            ui.label(
                egui::RichText::new(self.config_path.display().to_string())
                    .monospace()
                    .small()
                    .color(egui::Color32::from_rgb(150, 160, 190)),
            );
            ui.add_space(8.0);
            if ui.button("Open in file manager").clicked() {
                if let Some(parent) = self.config_path.parent() {
                    let _ = open::that_detached(parent);
                }
            }
        });
    }
}
