// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_quick_select(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Quick select",
            "Label-hint quick-select mode: press the shortcut to overlay keyboard labels \
             on regex matches in the screen and scrollback. Type a label to copy its text. \
             Pane-select uses the same labels to focus a pane.",
        );

        let mut dirty = false;

        // ── Alphabet ────────────────────────────────────────────────────────
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Label alphabet");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.config.quick_select.alphabet)
                        .hint_text("asdfjklqwerzxcvghtybnuiopm")
                        .desired_width(260.0),
                );
                if resp.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.quick_select.alphabet =
                        terminale_config::QuickSelectConfig::default().alphabet;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::QuickSelect, "Label alphabet");
            sublabel(
                ui,
                "Characters used to generate label hints. Must be non-empty with no duplicates. \
                 Home-row-first alphabet gives the shortest key sequences.",
            );
            // Inline validation hint.
            if let Some(err) =
                terminale_config::quick_select_validate_alphabet(&self.config.quick_select.alphabet)
            {
                ui.colored_label(egui::Color32::from_rgb(220, 80, 60), err);
            }
        });

        ui.add_space(6.0);

        // ── Patterns list ───────────────────────────────────────────────────
        ui.label(
            egui::RichText::new("Regex patterns")
                .strong()
                .color(egui::Color32::from_rgb(140, 160, 200)),
        );
        ui.add_space(4.0);
        card(ui, |ui| {
            let hr = ui.vertical(|ui| {
                let mut to_remove: Option<usize> = None;
                let patterns = &mut self.config.quick_select.patterns;
                for (i, pat) in patterns.iter_mut().enumerate() {
                    let r = ui.horizontal(|ui| {
                        // Small remove button.
                        if ui.small_button("\u{2212}").clicked() {
                            to_remove = Some(i);
                        }
                        let resp = ui.add(
                            egui::TextEdit::singleline(pat)
                                .desired_width(ui.available_width() - 8.0),
                        );
                        if resp.changed() {
                            dirty = true;
                        }
                    });
                    // Inline error for invalid regex.
                    if regex::Regex::new(pat).is_err() {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 80, 60),
                            format!("Invalid regex: {pat}"),
                        );
                    }
                    let _ = r;
                }
                if let Some(idx) = to_remove {
                    self.config.quick_select.patterns.remove(idx);
                    dirty = true;
                }
                if ui.button("+ Add pattern").clicked() {
                    self.config.quick_select.patterns.push(String::new());
                    dirty = true;
                }
                if ui.small_button("Reset to defaults").clicked() {
                    self.config.quick_select.patterns =
                        terminale_config::QuickSelectConfig::default().patterns;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::QuickSelect, "Regex patterns");
        });

        ui.add_space(6.0);

        // ── Overlay dim ────────────────────────────────────────────────────
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Overlay dim");
                let resp = ui.add(
                    egui::Slider::new(&mut self.config.quick_select.overlay_dim, 0.0_f32..=1.0_f32)
                        .step_by(0.05)
                        .fixed_decimals(2),
                );
                if resp.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.quick_select.overlay_dim =
                        terminale_config::QuickSelectConfig::default().overlay_dim;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::QuickSelect, "Overlay dim");
            sublabel(
                ui,
                "Opacity of the full-screen tint drawn behind the label badges (0 = none, \
                 1 = fully opaque dark). Lower values keep more context visible.",
            );
        });

        ui.add_space(10.0);

        // ── Keybinds ────────────────────────────────────────────────────────
        ui.label(
            egui::RichText::new("Keybindings")
                .strong()
                .color(egui::Color32::from_rgb(140, 160, 200)),
        );
        ui.add_space(4.0);
        let ks_rows: &[(
            &str,
            &str,
            fn(&mut terminale_config::ShortcutsConfig) -> &mut String,
            &str,
        )] = &[
            (
                "quick_select",
                "Quick select",
                |s: &mut terminale_config::ShortcutsConfig| &mut s.quick_select,
                "Ctrl+Shift+Space",
            ),
            (
                "pane_select",
                "Pane select",
                |s: &mut terminale_config::ShortcutsConfig| &mut s.pane_select,
                "",
            ),
        ];
        card(ui, |ui| {
            for (id, label, getter, default) in ks_rows {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, label);
                    let binding = getter(&mut self.config.keybinds.shortcuts);
                    if hotkey_recorder(ui, id, binding, &mut self.recording_hotkey) {
                        dirty = true;
                    }
                    if ui.small_button("Reset").clicked() {
                        let b = getter(&mut self.config.keybinds.shortcuts);
                        *b = (*default).to_string();
                        dirty = true;
                    }
                    if ui.small_button("Clear").clicked() {
                        let b = getter(&mut self.config.keybinds.shortcuts);
                        b.clear();
                        dirty = true;
                    }
                });
                self.highlight_row(ui, hr.response.rect, Section::QuickSelect, label);
            }
        });

        if dirty {
            self.dirty = true;
        }
    }
}
