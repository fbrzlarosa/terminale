// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_snippets(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Snippets",
            "Named text snippets insertable into the active pane. \
             Open the picker via the command palette (\u{201C}Snippets\u{2026}\u{201D}) \
             or a configured keybind. \
             Bodies support \\n, \\t, \\e, \\xNN escape sequences.",
        );

        let mut dirty = false;
        let mut remove_idx: Option<usize> = None;

        // Iterate by index to avoid a simultaneous borrow of `self`.
        for idx in 0..self.config.snippets.len() {
            let name_display = if self.config.snippets[idx].name.is_empty() {
                "(unnamed)".to_string()
            } else {
                self.config.snippets[idx].name.clone()
            };

            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("\u{1F4CB}  {name_display}"))
                            .strong()
                            .color(egui::Color32::from_rgb(210, 220, 245)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("\u{1F5D1}")
                                        .color(egui::Color32::from_rgb(220, 130, 130)),
                                )
                                .fill(egui::Color32::from_rgb(40, 26, 30))
                                .rounding(0.0),
                            )
                            .on_hover_text("Remove this snippet")
                            .clicked()
                        {
                            remove_idx = Some(idx);
                        }
                    });
                });

                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    field_label(ui, "Name");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.config.snippets[idx].name)
                            .desired_width(320.0)
                            .hint_text("display name (e.g. Git status)"),
                    );
                    if r.changed() {
                        dirty = true;
                    }
                });

                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    field_label(ui, "Description");
                    let mut desc = self.config.snippets[idx]
                        .description
                        .clone()
                        .unwrap_or_default();
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut desc)
                            .desired_width(360.0)
                            .hint_text("one-line description shown in the picker (optional)"),
                    );
                    if r.changed() {
                        self.config.snippets[idx].description =
                            if desc.is_empty() { None } else { Some(desc) };
                        dirty = true;
                    }
                });

                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    field_label(ui, "Body");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.config.snippets[idx].body)
                            .desired_width(460.0)
                            .hint_text(r"text to insert — supports \n \t \e \xNN")
                            .font(egui::TextStyle::Monospace),
                    );
                    if r.changed() {
                        dirty = true;
                    }
                });
            });
            ui.add_space(6.0);
        }

        if let Some(idx) = remove_idx {
            self.config.snippets.remove(idx);
            dirty = true;
        }

        // ── Add a new snippet ──────────────────────────────────────────────────
        if ui
            .add(
                egui::Button::new(
                    egui::RichText::new("  \u{2795}  Add snippet  ").color(egui::Color32::WHITE),
                )
                .fill(egui::Color32::from_rgb(40, 80, 150))
                .rounding(0.0),
            )
            .clicked()
        {
            self.config.snippets.push(terminale_config::Snippet {
                name: String::new(),
                body: String::new(),
                description: None,
            });
            dirty = true;
        }

        if dirty {
            self.dirty = true;
        }
    }
}
