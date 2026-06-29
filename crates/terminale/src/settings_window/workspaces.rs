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

        // Refresh the cached workspace list (only scans disk when stale). The
        // body runs every frame while this tab is open, so it must not touch
        // the disk per-frame — see `cached_workspaces`.
        self.ensure_workspace_cache();

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
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Workspaces,
                "Restore working dirs",
            );
            sublabel(
                ui,
                "When restoring, open each shell in its last working directory \
                 (as announced by OSC 7). Disable to always start in the profile default.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Restore window geometry");
                let on = self.config.window.restore_window_geometry;
                if toggle_switch(ui, on).clicked() {
                    self.config.window.restore_window_geometry = !on;
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
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Workspaces,
                "Restore window geometry",
            );
            sublabel(
                ui,
                "When restoring, also bring back the window's position and size, the \
                 monitor it was on, and reopen in Quake mode if it was closed that way. \
                 Disable to restore only the tab/pane layout at the default geometry.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let on = self.config.window.session_autosave_secs > 0;
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Autosave session");
                if toggle_switch(ui, on).clicked() {
                    // Off stores 0 (save on close only); on restores the default cadence.
                    self.config.window.session_autosave_secs = if on { 0 } else { 15 };
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
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Workspaces,
                "Autosave session",
            );
            if self.config.window.session_autosave_secs > 0 {
                let hr2 = ui.horizontal(|ui| {
                    field_label(ui, "Autosave interval");
                    let r = ui.add(
                        egui::Slider::new(&mut self.config.window.session_autosave_secs, 5..=300)
                            .suffix(" s")
                            .text(""),
                    );
                    if r.changed() {
                        dirty = true;
                    }
                    if ui.small_button("Reset").clicked() {
                        self.config.window.session_autosave_secs = 15;
                        dirty = true;
                    }
                });
                self.highlight_row(
                    ui,
                    hr2.response.rect,
                    Section::Workspaces,
                    "Autosave interval",
                );
            }
            sublabel(
                ui,
                "Periodically save the last session to disk so a crash or power loss can \
                 restore your tabs — not just a clean exit. Layout only (no running \
                 processes). Disable to save on close only.",
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

        let hr_list = ui.horizontal(|ui| {
            field_label(ui, "Workspaces list");
        });
        self.highlight_row(
            ui,
            hr_list.response.rect,
            Section::Workspaces,
            "Workspaces list",
        );

        if self.cached_workspaces.is_empty() {
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
            // Track a delete during iteration and invalidate the cache after
            // the loop — we can't mutate `self` while borrowing the list.
            let mut deleted = false;
            for (name, path) in &self.cached_workspaces {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(name)
                            .color(egui::Color32::from_rgb(220, 230, 255))
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button(
                                egui::RichText::new("Delete")
                                    .color(egui::Color32::from_rgb(220, 90, 90)),
                            )
                            .on_hover_text("Delete this workspace permanently")
                            .clicked()
                        {
                            let _ = std::fs::remove_file(path);
                            deleted = true;
                        }
                    });
                });
                ui.add_space(2.0);
            }
            if deleted {
                self.workspace_cache_dirty = true;
            }
        }

        if dirty {
            self.dirty = true;
        }
    }
}
