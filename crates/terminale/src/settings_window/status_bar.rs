// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_status_bar(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Status bar",
            "Configurable status bar: a strip at the top or bottom of the terminal \
             showing cwd, clock, active profile, tab index, user vars, and literal text.",
        );

        let mut dirty = false;

        // ── Enable toggle ───────────────────────────────────────────────────
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Enable status bar");
                let on = self.config.status_bar.enabled;
                if toggle_switch(ui, on).clicked() {
                    self.config.status_bar.enabled = !on;
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
                Section::StatusBar,
                "Enable status bar",
            );
            sublabel(
                ui,
                "Show a one-row info strip at the top or bottom edge of each terminal window.",
            );
        });

        ui.add_space(6.0);

        // ── Position ────────────────────────────────────────────────────────
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Status bar position");
                egui::ComboBox::from_id_salt("status_bar_position_combo")
                    .selected_text(self.config.status_bar.position.label())
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for pos in terminale_config::StatusBarPosition::all() {
                            if ui
                                .selectable_value(
                                    &mut self.config.status_bar.position,
                                    pos,
                                    pos.label(),
                                )
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::StatusBar,
                "Status bar position",
            );
            sublabel(ui, "Top draws the bar between the tab bar and the terminal body; Bottom pins it to the window's lower edge.");
        });

        ui.add_space(6.0);

        // ── Update interval ─────────────────────────────────────────────────
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Update interval");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.status_bar.update_interval_ms, 200..=10000)
                        .suffix(" ms")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.status_bar.update_interval_ms =
                        terminale_config::StatusBarConfig::default().update_interval_ms;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::StatusBar, "Update interval");
            sublabel(
                ui,
                "How often (milliseconds) the bar is refreshed. Relevant when a Clock segment is active.",
            );
        });

        ui.add_space(6.0);

        // ── Left segments ───────────────────────────────────────────────────
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Left segments");
            });
            self.highlight_row(ui, hr.response.rect, Section::StatusBar, "Left segments");
            sublabel(
                ui,
                "Segments displayed on the left side. Available kinds: cwd, clock, profile, tab_index, user_var, literal, spacer.",
            );
            ui.add_space(4.0);
            Self::segment_editor(
                ui,
                &mut self.config.status_bar.left_segments,
                "sb_left",
                &mut dirty,
            );
        });

        ui.add_space(6.0);

        // ── Right segments ──────────────────────────────────────────────────
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Right segments");
            });
            self.highlight_row(ui, hr.response.rect, Section::StatusBar, "Right segments");
            sublabel(ui, "Segments displayed on the right side (right-aligned).");
            ui.add_space(4.0);
            Self::segment_editor(
                ui,
                &mut self.config.status_bar.right_segments,
                "sb_right",
                &mut dirty,
            );
        });

        if dirty {
            self.dirty = true;
        }
    }

    /// Inline editor for a `Vec<StatusSegment>` — shows each segment as a
    /// labelled row with a `Remove` button, and an `Add` dropdown at the bottom.
    pub(super) fn segment_editor(
        ui: &mut egui::Ui,
        segments: &mut Vec<terminale_config::StatusSegment>,
        id_prefix: &str,
        dirty: &mut bool,
    ) {
        use terminale_config::StatusSegment;

        let mut remove_idx: Option<usize> = None;
        for (i, seg) in segments.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(segment_kind_label(seg));
                // Editable param for segments with a string argument.
                match seg {
                    StatusSegment::Clock { format } => {
                        let r = ui.add(
                            egui::TextEdit::singleline(format)
                                .hint_text("%H:%M")
                                .desired_width(120.0),
                        );
                        if r.changed() {
                            *dirty = true;
                        }
                    }
                    StatusSegment::UserVar { name } => {
                        let r = ui.add(
                            egui::TextEdit::singleline(name)
                                .hint_text("variable name")
                                .desired_width(120.0),
                        );
                        if r.changed() {
                            *dirty = true;
                        }
                    }
                    StatusSegment::Literal { text } => {
                        let r = ui.add(
                            egui::TextEdit::singleline(text)
                                .hint_text("text")
                                .desired_width(160.0),
                        );
                        if r.changed() {
                            *dirty = true;
                        }
                    }
                    _ => {}
                }
                if ui.small_button("Remove").clicked() {
                    remove_idx = Some(i);
                    *dirty = true;
                }
            });
        }
        if let Some(i) = remove_idx {
            segments.remove(i);
        }

        // ── Add segment ──────────────────────────────────────────────────
        ui.horizontal(|ui| {
            // Use a combo to pick what kind to add.
            let kinds = [
                ("cwd", StatusSegment::Cwd),
                (
                    "clock",
                    StatusSegment::Clock {
                        format: "%H:%M".into(),
                    },
                ),
                ("profile", StatusSegment::Profile),
                ("tab_index", StatusSegment::TabIndex),
                (
                    "user_var",
                    StatusSegment::UserVar {
                        name: "var_name".into(),
                    },
                ),
                ("literal", StatusSegment::Literal { text: " | ".into() }),
                ("spacer", StatusSegment::Spacer),
            ];
            let combo_id = format!("{id_prefix}_add_combo");
            egui::ComboBox::from_id_salt(combo_id)
                .selected_text("+ add segment")
                .width(160.0)
                .show_ui(ui, |ui| {
                    for (label, template) in &kinds {
                        if ui.button(*label).clicked() {
                            segments.push(template.clone());
                            *dirty = true;
                        }
                    }
                });
        });
    }
}
