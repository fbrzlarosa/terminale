// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_cursor(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Cursor",
            "Shape, blink rate, colour, and on-cell tint of the typing caret.",
        );

        let mut dirty = false;

        // Style picker.
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Style");
                egui::ComboBox::from_id_salt("cursor_style_combo")
                    .selected_text(self.config.cursor.style.label())
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for s in terminale_config::CursorStyle::all() {
                            if ui
                                .selectable_value(&mut self.config.cursor.style, s, s.label())
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Cursor, "Style");
            sublabel(
                ui,
                "Block: filled. Outline: hollow. Underline: bottom bar. Beam: vertical I-beam.",
            );
        });

        ui.add_space(6.0);

        // Blink.
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Blink");
                if toggle_switch(ui, self.config.cursor.blink).clicked() {
                    self.config.cursor.blink = !self.config.cursor.blink;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Cursor, "Blink");
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Rate");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.cursor.blink_rate_ms, 80..=2000)
                        .suffix(" ms")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Cursor, "Rate");
            sublabel(
                ui,
                "Half-period of the on/off cycle. Lower = faster. Most terminals default to ~530 ms.",
            );

            ui.add_space(4.0);

            let hr = ui.horizontal(|ui| {
                field_label(ui, "Blink ease");
                let on = self.config.cursor.blink_ease;
                if toggle_switch(ui, on).clicked() {
                    self.config.cursor.blink_ease = !on;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Cursor, "Blink ease");
            sublabel(
                ui,
                "Smooth fade-in/fade-out using a smoothstep curve instead of hard on/off switching.",
            );

            let blink_ease_on = self.config.cursor.blink_ease;
            ui.add_enabled_ui(blink_ease_on, |ui| {
                ui.add_space(4.0);
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Animation FPS");
                    let r = ui.add(
                        egui::Slider::new(&mut self.config.cursor.animation_fps, 10..=240)
                            .suffix(" fps")
                            .text(""),
                    );
                    if r.changed() {
                        dirty = true;
                    }
                });
                self.highlight_row(ui, hr.response.rect, Section::Cursor, "Animation FPS");
                sublabel(ui, "Target frame rate for the eased blink animation.");
            });
        });

        ui.add_space(6.0);

        // Geometry: thickness + opacity + tint.
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Thickness");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.cursor.thickness_px, 0.5..=6.0)
                        .step_by(0.1)
                        .suffix(" px")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Cursor, "Thickness");
            sublabel(ui, "Stroke width for Underline / Beam / Outline styles.");
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Opacity");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.cursor.opacity, 0.0..=1.0)
                        .step_by(0.01)
                        .custom_formatter(|v, _| format!("{:.0} %", v * 100.0))
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Cursor, "Opacity");
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Cell tint");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.cursor.cell_tint_opacity, 0.0..=0.6)
                        .step_by(0.01)
                        .custom_formatter(|v, _| format!("{:.0} %", v * 100.0))
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Cursor, "Cell tint");
            sublabel(
                ui,
                "Tints the entire cell behind the cursor — useful for high-contrast caret highlight.",
            );
        });

        ui.add_space(6.0);

        // Colour: optional override.
        card(ui, |ui| {
            let mut use_custom = self.config.cursor.color.is_some();
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Custom colour");
                if toggle_switch(ui, use_custom).clicked() {
                    use_custom = !use_custom;
                    if use_custom {
                        self.config.cursor.color = Some([0x7d, 0xa6, 0xff]);
                    } else {
                        self.config.cursor.color = None;
                    }
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Cursor, "Custom colour");
            if let Some(rgb) = self.config.cursor.color.as_mut() {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Colour");
                    let mut color = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                    if ui.color_edit_button_srgba(&mut color).changed() {
                        rgb[0] = color.r();
                        rgb[1] = color.g();
                        rgb[2] = color.b();
                        dirty = true;
                    }
                });
                self.highlight_row(ui, hr.response.rect, Section::Cursor, "Colour");
            }
            sublabel(
                ui,
                "When off, the cursor uses the active theme's accent colour.",
            );
        });

        if dirty {
            self.dirty = true;
        }
    }
}