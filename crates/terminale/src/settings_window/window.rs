// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_window(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "Window", "Transparency, padding, and window behaviour.");

        let mut dirty = false;

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Opacity");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.window.opacity, 0.5..=1.0)
                        .step_by(0.01)
                        .custom_formatter(|v, _| format!("{:.0} %", v * 100.0))
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.window.opacity = 1.0;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Window, "Opacity");
            sublabel(
                ui,
                "Below 100% the window background blends with what's behind it.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Padding");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.window.padding, 0..=64)
                        .suffix(" px")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.window.padding = 8;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Window, "Padding");
            sublabel(ui, "Space between the window edge and the terminal grid.");
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Confirm close");
                let on = self.config.window.confirm_close;
                if toggle_switch(ui, on).clicked() {
                    self.config.window.confirm_close = !on;
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
            self.highlight_row(ui, hr.response.rect, Section::Window, "Confirm close");
            sublabel(
                ui,
                "Show a confirmation dialog before a tab or the window closes. \
                 Close confirms, Cancel/Esc keeps everything open.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Stay on top");
                let on = self.config.window.always_on_top;
                if toggle_switch(ui, on).clicked() {
                    self.config.window.always_on_top = !on;
                    // Pin / unpin Settings live so it tracks the choice
                    // immediately, not just at next open.
                    self.apply_own_window_level();
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
            self.highlight_row(ui, hr.response.rect, Section::Window, "Stay on top");
            sublabel(
                ui,
                "Keep the window above all others. Settings is pinned with the terminal.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Startup position");
                let current = self.config.window.startup_position;
                let label = match current {
                    None => "Default (OS)",
                    Some(edge) => edge.label(),
                };
                egui::ComboBox::from_id_salt("startup_position")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(current.is_none(), "Default (OS)")
                            .clicked()
                        {
                            self.config.window.startup_position = None;
                            dirty = true;
                        }
                        for edge in terminale_config::SnapEdge::all() {
                            if ui
                                .selectable_label(current == Some(edge), edge.label())
                                .clicked()
                            {
                                self.config.window.startup_position = Some(edge);
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Window, "Startup position");
            sublabel(
                ui,
                "Where the window opens on launch. The Snap shortcuts \
                 (halves, quarters, center, maximize) reposition an already-open \
                 window — bind them in Shortcuts, or use Show Snap Layouts.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Auto reload config");
                let on = self.config.window.auto_reload_config;
                if toggle_switch(ui, on).clicked() {
                    self.config.window.auto_reload_config = !on;
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
            self.highlight_row(ui, hr.response.rect, Section::Window, "Auto reload config");
            sublabel(
                ui,
                "Watch config.toml for external edits and live-apply them automatically. \
                 Use 'Reload Config' in the command palette or bind a shortcut for a \
                 manual reload.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "New-window profile");
                let current = self.config.window.new_window_profile.clone();
                let label = current.as_deref().unwrap_or("Default");
                egui::ComboBox::from_id_salt("new_window_profile")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "Default").clicked() {
                            self.config.window.new_window_profile = None;
                            dirty = true;
                        }
                        let profile_names: Vec<String> = self
                            .config
                            .profiles
                            .profiles
                            .iter()
                            .map(|p| p.name.clone())
                            .collect();
                        for name in &profile_names {
                            if ui
                                .selectable_label(
                                    current.as_deref() == Some(name.as_str()),
                                    name.as_str(),
                                )
                                .clicked()
                            {
                                self.config.window.new_window_profile = Some(name.clone());
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Window, "New-window profile");
            sublabel(
                ui,
                "Profile for the first tab when opening a new window (Ctrl+Shift+N or \
                 'New Window' in the command palette). 'Default' uses the same profile \
                 as Ctrl+T.",
            );
        });

        ui.add_space(6.0);

        // ── Zen mode ────────────────────────────────────────────────────────────
        ui.label(
            egui::RichText::new("Zen mode")
                .strong()
                .color(egui::Color32::from_rgb(140, 160, 200)),
        );
        ui.add_space(4.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Enter full-screen");
                let on = self.config.window.zen_fullscreen;
                if toggle_switch(ui, on).clicked() {
                    self.config.window.zen_fullscreen = !on;
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
            self.highlight_row(ui, hr.response.rect, Section::Window, "Enter full-screen");
            sublabel(
                ui,
                "When zen mode activates, also enter borderless full-screen. Exiting zen restores the prior windowed state.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            ui.label(
                egui::RichText::new("Chrome hidden in zen mode")
                    .small()
                    .color(egui::Color32::from_rgb(160, 170, 200)),
            );
            ui.add_space(4.0);
            for element in terminale_config::ZenHideElement::all() {
                let present = self.config.window.zen_hide.iter().any(|e| e == &element);
                let hr = ui.horizontal(|ui| {
                    let mut checked = present;
                    if ui.checkbox(&mut checked, element.label()).changed() {
                        if checked {
                            if !present {
                                self.config.window.zen_hide.push(element.clone());
                            }
                        } else {
                            self.config.window.zen_hide.retain(|e| e != &element);
                        }
                        dirty = true;
                    }
                });
                self.highlight_row(ui, hr.response.rect, Section::Window, element.label());
            }
            sublabel(
                ui,
                "Check each element you want hidden while zen mode is active.",
            );
        });

        if dirty {
            self.dirty = true;
        }
    }
}
