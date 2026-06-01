// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_directory_jump(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Directory Jump",
            "Tracks visited directories via OSC 7 and surfaces a frecency-ranked fuzzy picker.",
        );

        let mut dirty = false;

        // ── Enable / disable ─────────────────────────────────────────────────

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Enable directory jump");
                let on = self.config.directory_jump.enabled;
                if toggle_switch(ui, on).clicked() {
                    self.config.directory_jump.enabled = !on;
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
                Section::DirectoryJump,
                "Enable directory jump",
            );
            sublabel(
                ui,
                "When enabled, each OSC 7 cwd report is recorded in the frecency store. \
                 Requires a shell that emits OSC 7 on directory changes (zsh, bash with \
                 $PROMPT_COMMAND, fish, PowerShell with a cd wrapper, etc.).",
            );
        });

        ui.add_space(6.0);

        // ── Max tracked ───────────────────────────────────────────────────────

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Max tracked directories");
                let r = ui.add(
                    egui::Slider::new(
                        &mut self.config.directory_jump.max_tracked,
                        1_usize..=2000_usize,
                    )
                    .suffix(" dirs")
                    .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.directory_jump.max_tracked = 200;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::DirectoryJump,
                "Max tracked directories",
            );
            sublabel(
                ui,
                "Maximum number of directory entries the store keeps. When the cap is \
                 reached, the entry with the lowest frecency score is evicted.",
            );
        });

        ui.add_space(6.0);

        // ── Persist to disk ───────────────────────────────────────────────────

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Persist history to disk");
                let on = self.config.directory_jump.persist;
                if toggle_switch(ui, on).clicked() {
                    self.config.directory_jump.persist = !on;
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
                Section::DirectoryJump,
                "Persist history to disk",
            );
            sublabel(
                ui,
                "When enabled, the visit history is saved to \
                 <data_dir>/dir_history.toml and restored on next launch. \
                 When disabled, the store is memory-only and resets on exit.",
            );
        });

        if dirty {
            self.dirty = true;
        }
    }
}
