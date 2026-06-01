// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_context_rules(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Context rules",
            "Automatically tint a tab and/or show a badge when the connected SSH host or current \
             working directory matches a glob. Rules are evaluated in order; the first match wins. \
             The primary use case is a safety cue: production hosts get a red tab so you don't \
             fat-finger a destructive command on prod.",
        );

        let mut dirty = false;
        let mut remove_idx: Option<usize> = None;

        for idx in 0..self.config.context_rules.len() {
            let name_display = if self.config.context_rules[idx].name.is_empty() {
                "(unnamed rule)".to_string()
            } else {
                self.config.context_rules[idx].name.clone()
            };

            card(ui, |ui| {
                // ── Header row: name + remove button ────────────────────────
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("\u{1F3F7}  {name_display}"))
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
                            .on_hover_text("Remove this rule")
                            .clicked()
                        {
                            remove_idx = Some(idx);
                        }
                    });
                });

                ui.add_space(8.0);

                // ── Rule name ───────────────────────────────────────────────
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Rule name");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.config.context_rules[idx].name)
                            .desired_width(320.0)
                            .hint_text("display name (e.g. Production)"),
                    );
                    if r.changed() {
                        dirty = true;
                    }
                });
                self.highlight_row(ui, hr.response.rect, Section::ContextRules, "Rule name");

                ui.add_space(4.0);

                // ── Host glob ───────────────────────────────────────────────
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Host glob");
                    let mut host = self.config.context_rules[idx]
                        .host_glob
                        .clone()
                        .unwrap_or_default();
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut host)
                            .desired_width(300.0)
                            .hint_text("e.g. *prod* (leave empty to skip host matching)"),
                    );
                    if r.changed() {
                        self.config.context_rules[idx].host_glob =
                            if host.is_empty() { None } else { Some(host) };
                        dirty = true;
                    }
                });
                self.highlight_row(ui, hr.response.rect, Section::ContextRules, "Host glob");

                ui.add_space(4.0);

                // ── Cwd glob ────────────────────────────────────────────────
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Cwd glob");
                    let mut cwd = self.config.context_rules[idx]
                        .cwd_glob
                        .clone()
                        .unwrap_or_default();
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut cwd)
                            .desired_width(340.0)
                            .hint_text("e.g. /srv/production/* (leave empty to skip cwd matching)"),
                    );
                    if r.changed() {
                        self.config.context_rules[idx].cwd_glob =
                            if cwd.is_empty() { None } else { Some(cwd) };
                        dirty = true;
                    }
                });
                self.highlight_row(ui, hr.response.rect, Section::ContextRules, "Cwd glob");

                ui.add_space(4.0);

                // ── Tab color picker (RGB sliders) ───────────────────────────
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Tab color");
                    let color_on = self.config.context_rules[idx].tab_color.is_some();
                    if toggle_switch(ui, color_on).clicked() {
                        if color_on {
                            self.config.context_rules[idx].tab_color = None;
                        } else {
                            self.config.context_rules[idx].tab_color = Some([200, 50, 50]);
                        }
                        dirty = true;
                    }
                    if let Some(ref mut rgb) = self.config.context_rules[idx].tab_color {
                        ui.add_space(8.0);
                        // Show a colour preview swatch.
                        let swatch_rect = ui.allocate_space(egui::vec2(24.0, 14.0)).1;
                        ui.painter().rect_filled(
                            swatch_rect,
                            2.0,
                            egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                        );
                        ui.add_space(4.0);
                        let mut r32 = rgb[0] as f32;
                        let mut g32 = rgb[1] as f32;
                        let mut b32 = rgb[2] as f32;
                        ui.label(egui::RichText::new("R").strong());
                        let sr = ui.add(
                            egui::Slider::new(&mut r32, 0.0..=255.0)
                                .show_value(true)
                                .fixed_decimals(0),
                        );
                        ui.label(egui::RichText::new("G").strong());
                        let sg = ui.add(
                            egui::Slider::new(&mut g32, 0.0..=255.0)
                                .show_value(true)
                                .fixed_decimals(0),
                        );
                        ui.label(egui::RichText::new("B").strong());
                        let sb = ui.add(
                            egui::Slider::new(&mut b32, 0.0..=255.0)
                                .show_value(true)
                                .fixed_decimals(0),
                        );
                        if sr.changed() || sg.changed() || sb.changed() {
                            rgb[0] = r32 as u8;
                            rgb[1] = g32 as u8;
                            rgb[2] = b32 as u8;
                            dirty = true;
                        }
                    }
                });
                self.highlight_row(ui, hr.response.rect, Section::ContextRules, "Tab color");
                sublabel(
                    ui,
                    "RGB tint applied to the tab chip when this rule matches. \
                     Blended at 40 % over the default background so text stays readable.",
                );

                ui.add_space(4.0);

                // ── Badge text ──────────────────────────────────────────────
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Badge");
                    let mut badge = self.config.context_rules[idx]
                        .badge
                        .clone()
                        .unwrap_or_default();
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut badge)
                            .desired_width(120.0)
                            .hint_text("e.g. PROD (up to ~6 chars)"),
                    );
                    if r.changed() {
                        self.config.context_rules[idx].badge =
                            if badge.is_empty() { None } else { Some(badge) };
                        dirty = true;
                    }
                });
                self.highlight_row(ui, hr.response.rect, Section::ContextRules, "Badge");
                sublabel(
                    ui,
                    "Short text overlaid on the tab pill (e.g. \u{201C}PROD\u{201D}, \
                     \u{201C}STG\u{201D}). Keep to 6 characters or fewer for best fit.",
                );
            });
            ui.add_space(6.0);
        }

        if let Some(idx) = remove_idx {
            self.config.context_rules.remove(idx);
            dirty = true;
        }

        // ── Add a new rule ─────────────────────────────────────────────────
        if ui
            .add(
                egui::Button::new(
                    egui::RichText::new("  \u{2795}  Add context rule  ")
                        .color(egui::Color32::WHITE),
                )
                .fill(egui::Color32::from_rgb(40, 80, 150))
                .rounding(0.0),
            )
            .clicked()
        {
            self.config.context_rules.push(terminale_config::ContextRule {
                name: String::new(),
                host_glob: None,
                cwd_glob: None,
                tab_color: Some([200, 50, 50]),
                badge: None,
            });
            dirty = true;
        }

        if dirty {
            self.dirty = true;
        }
    }
}
