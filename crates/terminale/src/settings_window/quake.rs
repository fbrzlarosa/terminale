// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_quake(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Quake mode",
            "Press the global hotkey to show/hide the window. Dock it to a \
             screen edge for an edge-docked drop-down, or leave it free-floating \
             to restore the last position on every show.",
        );
        sublabel(
            ui,
            "Window-level options (Stay on top, Startup position, Opacity, Padding, \
             Confirm close) are in the Window section.",
        );

        let mut dirty = false;

        // ── Hotkey ──
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Global hotkey");
                if hotkey_recorder(
                    ui,
                    "quake",
                    &mut self.config.keybinds.quake,
                    &mut self.recording_hotkey,
                ) {
                    dirty = true;
                }
                if ui.small_button("Disable").clicked() {
                    self.config.keybinds.quake.clear();
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Quake, "Global hotkey");
            sublabel(
                ui,
                "Click the button, press the combo you want. Esc cancels. \
                 Empty = Quake disabled. (requires restart to change the hotkey)",
            );
        });

        ui.add_space(6.0);

        // ── Dock edge (Off / Top / Bottom / Left / Right) ──
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Dock to edge");
                ui.horizontal(|ui| {
                    for edge in terminale_config::QuakeEdge::all() {
                        let selected = self.config.quake.edge == edge;
                        if ui.selectable_label(selected, edge.label()).clicked() {
                            self.config.quake.edge = edge;
                            dirty = true;
                        }
                    }
                });
            });
            self.highlight_row(ui, hr.response.rect, Section::Quake, "Dock to edge");
            sublabel(
                ui,
                "Off keeps the window wherever you last left it (exact-geometry \
                 restore on every show). The four edges snap to that side of the \
                 chosen monitor at the configured size and margin.",
            );
        });

        ui.add_space(6.0);

        // ── Display picker — only meaningful when docked ──
        let docked = self.config.quake.edge != terminale_config::QuakeEdge::Off;
        if docked {
            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Display");

                    // Build friendly hints for "Current" and "Primary" once
                    // per frame so both the selected-text and the dropdown
                    // entries show the physical monitor name.
                    //
                    // "Current" is resolved from the OS cursor position
                    // (same logic as the hotkey handler uses at runtime), so
                    // the hint always shows the monitor the cursor is on right
                    // now — independent of where the Settings window is.
                    let monitors: Vec<_> = self.window.available_monitors().collect();
                    let cursor_monitor = crate::monitor_names::os_cursor_position()
                        .and_then(|p| crate::monitor_names::monitor_at_point(&monitors, p));
                    let current_monitor = self.window.current_monitor();
                    let current_hint = cursor_monitor
                        .as_ref()
                        .or(current_monitor.as_ref())
                        .map_or_else(
                            || "unknown".to_string(),
                            |m| crate::monitor_names::friendly_monitor_label(m, 0),
                        );
                    let os_primary = crate::monitor_names::os_primary_monitor(&monitors);
                    let winit_primary = self.window.primary_monitor();
                    let primary_hint = os_primary.as_ref().or(winit_primary.as_ref()).map_or_else(
                        || "unknown".to_string(),
                        |m| crate::monitor_names::friendly_monitor_label(m, 0),
                    );

                    let current_label = match self.config.quake.display {
                        terminale_config::QuakeDisplay::Current => {
                            format!("Current \u{2014} {current_hint}")
                        }
                        terminale_config::QuakeDisplay::Primary => {
                            format!("Primary \u{2014} {primary_hint}")
                        }
                        terminale_config::QuakeDisplay::Index(i) => format!("Display {}", i + 1),
                    };
                    egui::ComboBox::from_id_salt("quake_display_combo")
                        .selected_text(current_label)
                        .width(280.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(
                                    matches!(
                                        self.config.quake.display,
                                        terminale_config::QuakeDisplay::Current
                                    ),
                                    format!("Current \u{2014} {current_hint}"),
                                )
                                .clicked()
                            {
                                self.config.quake.display = terminale_config::QuakeDisplay::Current;
                                dirty = true;
                            }
                            if ui
                                .selectable_label(
                                    matches!(
                                        self.config.quake.display,
                                        terminale_config::QuakeDisplay::Primary
                                    ),
                                    format!("Primary \u{2014} {primary_hint}"),
                                )
                                .clicked()
                            {
                                self.config.quake.display = terminale_config::QuakeDisplay::Primary;
                                dirty = true;
                            }
                            // Enumerate the monitors the Settings window
                            // currently sees. Each entry pins Quake to that
                            // index; falls back gracefully if a previously-
                            // chosen index is no longer present.
                            // `friendly_monitor_label` resolves the OS-
                            // supplied name (e.g. "BenQ EW3270U") and falls
                            // back to "Display N (WxH)" — never shows raw
                            // GDI paths like \\.\DISPLAY1.
                            for (idx, mon) in monitors.iter().enumerate() {
                                if idx > 7 {
                                    break;
                                }
                                let i = idx as u8;
                                let label = crate::monitor_names::friendly_monitor_label(mon, idx);
                                if ui
                                    .selectable_label(
                                        matches!(
                                            self.config.quake.display,
                                            terminale_config::QuakeDisplay::Index(j) if j == i
                                        ),
                                        label,
                                    )
                                    .clicked()
                                {
                                    self.config.quake.display =
                                        terminale_config::QuakeDisplay::Index(i);
                                    dirty = true;
                                }
                            }
                        });
                });
                self.highlight_row(ui, hr.response.rect, Section::Quake, "Display");
                sublabel(
                    ui,
                    "Which monitor the dock attaches to. \
                     \u{201c}Current\u{201d} = the monitor containing your mouse \
                     cursor at the moment you press the Quake hotkey \u{2014} \
                     queried from the OS at hotkey time, independent of where \
                     this Settings window or the terminal window is. \
                     The name shown updates live as you move your cursor. \
                     \u{201c}Primary\u{201d} always uses the OS-marked primary.",
                );
            });

            ui.add_space(6.0);

            // ── Size + margin ──
            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Size");
                    let r = ui.add(
                        egui::Slider::new(&mut self.config.quake.size_percent, 0.1..=1.0)
                            .step_by(0.01)
                            .custom_formatter(|v, _| format!("{:.0} %", v * 100.0))
                            .text(""),
                    );
                    if r.changed() {
                        dirty = true;
                    }
                });
                self.highlight_row(ui, hr.response.rect, Section::Quake, "Size");
                sublabel(ui, "Fraction of the monitor the docked window covers.");
            });

            ui.add_space(6.0);

            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Margin");
                    let r = ui.add(
                        egui::Slider::new(&mut self.config.quake.margin_px, 0..=200)
                            .suffix(" px")
                            .text(""),
                    );
                    if r.changed() {
                        dirty = true;
                    }
                });
                self.highlight_row(ui, hr.response.rect, Section::Quake, "Margin");
                sublabel(ui, "Inset along the dock edge from the screen edge.");
            });

            ui.add_space(6.0);

            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Hide on focus loss");
                    let on = self.config.quake.hide_on_focus_loss;
                    if toggle_switch(ui, on).clicked() {
                        self.config.quake.hide_on_focus_loss = !on;
                        dirty = true;
                    }
                });
                self.highlight_row(ui, hr.response.rect, Section::Quake, "Hide on focus loss");
                sublabel(
                    ui,
                    "Slide the docked window away when it loses focus — \
                     auto-hide on focus loss.",
                );
            });

            ui.add_space(6.0);
        }

        // ── Animation ──
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Animation");
                egui::ComboBox::from_id_salt("quake_animation_combo")
                    .selected_text(self.config.quake.animation.label())
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for a in terminale_config::QuakeAnimation::all() {
                            if ui
                                .selectable_value(&mut self.config.quake.animation, a, a.label())
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Quake, "Animation");
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Duration");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.quake.animation_ms, 0..=600)
                        .suffix(" ms")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Quake, "Duration");
            sublabel(
                ui,
                "Slide and Bounce animate the OS window geometry by moving the window in/out from \
                 the dock edge — smooth and zero-content-flicker. Bounce adds a spring overshoot. \
                 Scale also resizes the window each frame (may be less smooth on Windows). \
                 None is instant.",
            );
        });

        if dirty {
            self.dirty = true;
        }
    }
}