// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

/// Standard `tracing` level filters offered in the level dropdown. Users can
/// still hand-edit `logging.file_level` in the TOML to a finer directive such
/// as `terminale=debug`; such a value shows verbatim in the combo box.
const LOG_LEVELS: &[&str] = &["error", "warn", "info", "debug", "trace"];

impl SettingsWindow {
    pub(super) fn section_logging(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Logging",
            "Diagnostic log file and the freeze watchdog. The log is what lets a crash \
             or a momentary freeze leave evidence to investigate afterwards — a GUI \
             launch has no console of its own.",
        );

        let mut dirty = false;

        // ── Log file ───────────────────────────────────────────────────────────
        ui.label(
            egui::RichText::new("Log file")
                .strong()
                .color(egui::Color32::from_rgb(140, 160, 200)),
        );
        ui.add_space(4.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Write log file");
                let on = self.config.logging.file_enabled;
                if toggle_switch(ui, on).clicked() {
                    self.config.logging.file_enabled = !on;
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
            self.highlight_row(ui, hr.response.rect, Section::Logging, "Write log file");
            sublabel(
                ui,
                "Write a rolling daily log to <config dir>/logs/. Without it a freeze \
                 or crash leaves nothing to inspect. Takes effect on the next restart.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Log level");
                let current = self.config.logging.file_level.clone();
                egui::ComboBox::from_id_salt("log_file_level")
                    .selected_text(current.as_str())
                    .show_ui(ui, |ui| {
                        for lvl in LOG_LEVELS {
                            if ui.selectable_label(current == *lvl, *lvl).clicked() {
                                self.config.logging.file_level = (*lvl).to_owned();
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Logging, "Log level");
            sublabel(
                ui,
                "Verbosity of the log file. 'info' is a good default; 'debug' or 'trace' \
                 capture more when chasing a specific issue. Takes effect on the next restart.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Log retention");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.logging.retention_days, 1..=365)
                        .suffix(" days")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.logging.retention_days = 7;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Logging, "Log retention");
            sublabel(
                ui,
                "Delete log files older than this at startup. Takes effect on the next restart.",
            );
        });

        ui.add_space(12.0);

        // ── Freeze watchdog ─────────────────────────────────────────────────────
        ui.label(
            egui::RichText::new("Freeze watchdog")
                .strong()
                .color(egui::Color32::from_rgb(140, 160, 200)),
        );
        ui.add_space(4.0);

        card(ui, |ui| {
            let on = self.config.logging.slow_frame_warn_ms != 0;
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Warn on slow frames");
                if toggle_switch(ui, on).clicked() {
                    // Off stores 0 (disabled); on restores the default threshold.
                    self.config.logging.slow_frame_warn_ms = if on { 0 } else { 250 };
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
                Section::Logging,
                "Warn on slow frames",
            );
            if self.config.logging.slow_frame_warn_ms != 0 {
                let hr2 = ui.horizontal(|ui| {
                    field_label(ui, "Slow-frame threshold");
                    let r = ui.add(
                        egui::Slider::new(&mut self.config.logging.slow_frame_warn_ms, 16..=2000)
                            .suffix(" ms")
                            .text(""),
                    );
                    if r.changed() {
                        dirty = true;
                    }
                    if ui.small_button("Reset").clicked() {
                        self.config.logging.slow_frame_warn_ms = 250;
                        dirty = true;
                    }
                });
                self.highlight_row(
                    ui,
                    hr2.response.rect,
                    Section::Logging,
                    "Slow-frame threshold",
                );
            }
            sublabel(
                ui,
                "Log a warning when a single frame takes longer than this — it catches \
                 transient stalls (a GPU hitch, a blocking call on the UI thread) that \
                 recover on their own and otherwise leave no trace. Applies live.",
            );
        });

        if dirty {
            self.dirty = true;
        }
    }
}
