// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_font(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "Font", "Typeface, size, line height, and ligatures.");

        let mut dirty = false;

        // Offer the monospace fonts actually installed (reported by the
        // renderer) so every choice resolves instead of warning + falling back.
        // Fall back to the curated presets only until that list is populated.
        let font_choices: Vec<String> = if self.available_fonts.is_empty() {
            FONT_PRESETS.iter().map(|s| (*s).to_string()).collect()
        } else {
            self.available_fonts.clone()
        };

        // Helper: display label for a font family — appends "  (bundled)" when
        // the family is one of the typefaces embedded in the binary.
        // Clone the list into a local so the closures below don't capture
        // `self` by reference (they also need `&mut self` for highlight_row).
        let bundled: Vec<String> = self.bundled_fonts.clone();
        let family_label = |preset: &str| -> String {
            if bundled.iter().any(|b| b == preset) {
                format!("{preset}  (bundled)")
            } else {
                preset.to_string()
            }
        };

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Font family");
                let mut current_family = self.config.font.family.clone();
                let mut chosen: Option<String> = None;
                egui::ComboBox::from_id_salt("font_family_combo")
                    .selected_text(current_family.clone())
                    .width(280.0)
                    .show_ui(ui, |ui| {
                        for preset in &font_choices {
                            let label = family_label(preset);
                            if ui
                                .selectable_label(preset == &current_family, label.as_str())
                                .clicked()
                            {
                                chosen = Some(preset.clone());
                            }
                        }
                        ui.separator();
                        let r = ui.text_edit_singleline(&mut current_family);
                        if r.changed() {
                            chosen = Some(current_family.clone());
                        }
                    });
                if let Some(value) = chosen {
                    if value != self.config.font.family {
                        self.config.font.family = value;
                        dirty = true;
                    }
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Font, "Font family");
            sublabel(ui, "Any monospaced font installed on the system.");
        });

        ui.add_space(6.0);

        // ── Per-style font overrides ─────────────────────────────────────
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Bold font");
                // Empty string = "(derive from main)"
                let current_raw = self.config.font.bold_family.clone().unwrap_or_default();
                let mut current = current_raw.clone();
                let display = if current.is_empty() {
                    "(derive from main)".to_string()
                } else {
                    current.clone()
                };
                let mut chosen: Option<Option<String>> = None;
                egui::ComboBox::from_id_salt("bold_family_combo")
                    .selected_text(display)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(current.is_empty(), "(derive from main)")
                            .clicked()
                        {
                            chosen = Some(None);
                        }
                        for preset in &font_choices {
                            let label = family_label(preset);
                            if ui
                                .selectable_label(preset == &current, label.as_str())
                                .clicked()
                            {
                                chosen = Some(Some(preset.clone()));
                            }
                        }
                        ui.separator();
                        let r = ui.text_edit_singleline(&mut current);
                        if r.changed() {
                            chosen = Some(if current.is_empty() {
                                None
                            } else {
                                Some(current.clone())
                            });
                        }
                    });
                if let Some(value) = chosen {
                    if value != self.config.font.bold_family {
                        self.config.font.bold_family = value;
                        dirty = true;
                    }
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Font, "Bold font");
            sublabel(
                ui,
                "Separate font family for bold text. Leave blank to synthesize from the main family.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Italic font");
                let current_raw = self.config.font.italic_family.clone().unwrap_or_default();
                let mut current = current_raw.clone();
                let display = if current.is_empty() {
                    "(derive from main)".to_string()
                } else {
                    current.clone()
                };
                let mut chosen: Option<Option<String>> = None;
                egui::ComboBox::from_id_salt("italic_family_combo")
                    .selected_text(display)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(current.is_empty(), "(derive from main)")
                            .clicked()
                        {
                            chosen = Some(None);
                        }
                        for preset in &font_choices {
                            let label = family_label(preset);
                            if ui
                                .selectable_label(preset == &current, label.as_str())
                                .clicked()
                            {
                                chosen = Some(Some(preset.clone()));
                            }
                        }
                        ui.separator();
                        let r = ui.text_edit_singleline(&mut current);
                        if r.changed() {
                            chosen = Some(if current.is_empty() {
                                None
                            } else {
                                Some(current.clone())
                            });
                        }
                    });
                if let Some(value) = chosen {
                    if value != self.config.font.italic_family {
                        self.config.font.italic_family = value;
                        dirty = true;
                    }
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Font, "Italic font");
            sublabel(
                ui,
                "Separate font family for italic text. Leave blank to synthesize from the main family.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Bold-italic font");
                let current_raw =
                    self.config.font.bold_italic_family.clone().unwrap_or_default();
                let mut current = current_raw.clone();
                let display = if current.is_empty() {
                    "(derive from main)".to_string()
                } else {
                    current.clone()
                };
                let mut chosen: Option<Option<String>> = None;
                egui::ComboBox::from_id_salt("bold_italic_family_combo")
                    .selected_text(display)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(current.is_empty(), "(derive from main)")
                            .clicked()
                        {
                            chosen = Some(None);
                        }
                        for preset in &font_choices {
                            let label = family_label(preset);
                            if ui
                                .selectable_label(preset == &current, label.as_str())
                                .clicked()
                            {
                                chosen = Some(Some(preset.clone()));
                            }
                        }
                        ui.separator();
                        let r = ui.text_edit_singleline(&mut current);
                        if r.changed() {
                            chosen = Some(if current.is_empty() {
                                None
                            } else {
                                Some(current.clone())
                            });
                        }
                    });
                if let Some(value) = chosen {
                    if value != self.config.font.bold_italic_family {
                        self.config.font.bold_italic_family = value;
                        dirty = true;
                    }
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Font, "Bold-italic font");
            sublabel(
                ui,
                "Separate font family for bold-italic text. Falls back to bold or italic override, then main family.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Font size");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.font.size, 6.0..=48.0)
                        .step_by(0.5)
                        .suffix(" pt")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.font.size = 14.0;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Font, "Font size");
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Line height");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.font.line_height, 0.8..=3.0)
                        .step_by(0.05)
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.font.line_height = 1.25;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Font, "Line height");
            sublabel(ui, "Multiplier applied to the font size for each row.");
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Cell width");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.font.cell_width, 0.8_f32..=2.0)
                        .step_by(0.05)
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.font.cell_width = 1.0;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Font, "Cell width");
            sublabel(
                ui,
                "Multiplier applied to the monospace cell advance. Widens or narrows the grid without stretching glyphs.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Ligatures");
                let on = self.config.font.ligatures;
                if toggle_switch(ui, on).clicked() {
                    self.config.font.ligatures = !on;
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
            self.highlight_row(ui, hr.response.rect, Section::Font, "Ligatures");
            sublabel(
                ui,
                "Render programming ligatures (=>, !=, ===) when the font supports them.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Underline thickness");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.font.underline_thickness_px, 0.5_f32..=4.0)
                        .step_by(0.5)
                        .suffix(" px")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.font.underline_thickness_px = 1.0;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Font, "Underline thickness");
            sublabel(
                ui,
                "Stroke width (px) for SGR underlines — single, double, dotted, dashed, curly.",
            );
        });

        if dirty {
            self.dirty = true;
        }
    }
}