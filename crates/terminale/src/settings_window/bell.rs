// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_bell(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Bell",
            "How terminale reacts when an app emits BEL (\\x07).",
        );

        let mut dirty = false;
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Mode");
                egui::ComboBox::from_id_salt("bell_mode_combo")
                    .selected_text(bell_mode_label(self.config.bell.mode))
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for m in terminale_config::BellMode::all() {
                            if ui
                                .selectable_value(&mut self.config.bell.mode, m, bell_mode_label(m))
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Bell, "Mode");
            sublabel(
                ui,
                "Visual = window flash. Audio = system attention beep. Both fires both. None silences the bell.",
            );
        });

        if dirty {
            self.dirty = true;
        }
    }
}
