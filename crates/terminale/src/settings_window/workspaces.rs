// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_workspaces(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Workspaces",
            "Save and restore named layouts. Only the tab structure and working directories are restored — running processes are not.",
        );

        let mut dirty = false;

        // ── Session restore ───────────────────────────────────────────────────
        ui.label(
            egui::RichText::new("Session restore")
                .strong()
                .color(egui::Color32::from_rgb(140, 160, 200)),
        );
        ui.add_space(4.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Restore session");
                let current = self.config.window.restore_session;
                let label = current.label();
                egui::ComboBox::from_id_salt("restore_session")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        for variant in terminale_config::RestoreSession::all() {
                            if ui
                                .selectable_label(current == variant, variant.label())
                                .clicked()
                            {
                                self.config.window.restore_session = variant;
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Workspaces, "Restore session");
            sublabel(
                ui,
                "What to do on next launch. 'Restore last session' reopens the last \
                 set of tabs and splits (layout + directories only; no running processes).",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Restore working dirs");
                let on = self.config.window.restore_working_dirs;
                if toggle_switch(ui, on).clicked() {
                    self.config.window.restore_working_dirs = !on;
                    dirty = true;
                }
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(if on { "Enabled" } else { "Disabled" }).color(if on {
                        egui::Color32::from_rgb(120, 220, 140)
                    } else {
                        egui::Color32::from_rgb(140, 150, 175)
                    }),
                );
            });
            self.highlight_row(ui, hr.response.rect, Section::Workspaces, "Restore working dirs");
            sublabel(
                ui,
                "When restoring, open each shell in its last working directory \
                 (as announced by OSC 7). Disable to always start in the profile default.",
            );
        });

        ui.add_space(12.0);

        // ── Saved workspaces ─────────────────────────────────────────────────
        ui.label(
            egui::RichText::new("Saved workspaces")
                .strong()
                .color(egui::Color32::from_rgb(140, 160, 200)),
        );
        ui.add_space(4.0);

        let workspaces = terminale_config::paths::workspaces_dir()
            .map(|d| {
                std::fs::read_dir(&d)
                    .ok()
                    .map(|rd| {
                        let mut list: Vec<(String, std::path::PathBuf)> = rd
                            .flatten()
                            .filter_map(|e| {
                                let p = e.path();
                                if p.extension()? == "toml" {
                                    let name = p.file_stem()?.to_string_lossy().into_owned();
                                    Some((name, p))
                                } else {
                                    None
                                }
                            })
                            .collect();
                        list.sort_by(|a, b| a.0.cmp(&b.0));
                        list
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        let hr_list = ui.horizontal(|ui| {
            field_label(ui, "Workspaces list");
        });
        self.highlight_row(ui, hr_list.response.rect, Section::Workspaces, "Workspaces list");

        if workspaces.is_empty() {
            card(ui, |ui| {
                ui.label(
                    egui::RichText::new("No saved workspaces yet.")
                        .color(egui::Color32::from_rgb(130, 140, 165)),
                );
                sublabel(
                    ui,
                    "Use 'Save Workspace\u{2026}' in the command palette (Ctrl+Shift+P) \
                     to save the current layout.",
                );
            });
        } else {
            for (name, path) in &workspaces {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(name)
                            .color(egui::Color32::from_rgb(220, 230, 255))
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button(
                                egui::RichText::new("Delete").color(egui::Color32::from_rgb(220, 90, 90)),
                            )
                            .on_hover_text("Delete this workspace permanently")
                            .clicked()
                        {
                            let _ = std::fs::remove_file(path);
                        }
                    });
                });
                ui.add_space(2.0);
            }
        }

        if dirty {
            self.dirty = true;
        }
    }
}
