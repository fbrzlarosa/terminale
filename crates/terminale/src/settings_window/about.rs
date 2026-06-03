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
                 session — the new version applies on the next launch (never a forced restart).",
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
            if ui.add_enabled(!checking, egui::Button::new(label)).clicked() {
                // Runs in the background so the UI never blocks; the result is
                // sent back over a channel and surfaced as a visible status
                // line by `build_ui` (in addition to being logged).
                let (tx, rx) = std::sync::mpsc::channel();
                self.update_rx = Some(rx);
                self.status = Some((StatusKind::Success, "Checking for updates…".to_owned()));
                std::thread::spawn(move || {
                    let result = crate::update::download_and_stage().map_err(|e| format!("{e:#}"));
                    match &result {
                        Ok(Some(v)) => {
                            tracing::info!(version = %v, "update staged; restart to apply");
                        }
                        Ok(None) => tracing::info!("terminale is up to date"),
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
