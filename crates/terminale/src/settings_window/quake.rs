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

        // When a shell extension owns the drop-down, everything below the hotkey
        // is inert — the extension does the showing, the placing and the
        // animating, and the window it drives never goes through terminale's own
        // Quake path at all. Saying so here matters more than it looks: without
        // it the controls still move, still save, and still do nothing, which is
        // indistinguishable from a broken setting.
        #[cfg(target_os = "linux")]
        let extension_drives_quake = self.config.integration.quake_desktop_entry;
        #[cfg(not(target_os = "linux"))]
        let extension_drives_quake = false;
        if extension_drives_quake {
            card(ui, |ui| {
                ui.label(
                    egui::RichText::new(
                        "A shell extension is driving the drop-down \u{2014} these settings \
                         do not reach it",
                    )
                    .color(egui::Color32::from_rgb(230, 190, 120)),
                );
                sublabel(
                    ui,
                    "Desktop integration \u{203a} \"Drop-down via shell extension\" is on. The \
                     window that extension shows is placed, sized and animated by the \
                     extension, and never goes through terminale's own Quake mode \u{2014} so \
                     changing Animation or Duration here will not change what you see when you \
                     press its key. Adjust those in the extension's own settings. Everything \
                     here still applies to a drop-down terminale opens itself, on its own \
                     hotkey; turn the launcher off if you would rather terminale owned the \
                     drop-down again.",
                );
            });
            ui.add_space(6.0);
        }

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

                    // Build a friendly hint for "Primary" once per frame so
                    // both the selected-text and the dropdown entry show the
                    // physical monitor name. "Window's monitor" needs no
                    // hint: it resolves at toggle time to wherever the Quake
                    // window itself was last visible (which this Settings
                    // window cannot know).
                    let monitors: Vec<_> = self.window.available_monitors().collect();
                    let current_hint = "follows the window".to_string();
                    let os_primary = crate::monitor_names::os_primary_monitor(&monitors);
                    let winit_primary = self.window.primary_monitor();
                    let primary_hint = os_primary.as_ref().or(winit_primary.as_ref()).map_or_else(
                        || "unknown".to_string(),
                        |m| crate::monitor_names::friendly_monitor_label(m, 0),
                    );

                    let current_label = match self.config.quake.display {
                        terminale_config::QuakeDisplay::Current => {
                            format!("Window's monitor \u{2014} {current_hint}")
                        }
                        terminale_config::QuakeDisplay::Pointer => {
                            "Wherever the mouse is".to_string()
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
                                    format!("Window's monitor \u{2014} {current_hint}"),
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
                                        terminale_config::QuakeDisplay::Pointer
                                    ),
                                    "Wherever the mouse is",
                                )
                                .on_hover_text(
                                    "Opens on the monitor the pointer is on when you press \
                                     the hotkey. Needs X11 \u{2014} Wayland does not tell an \
                                     application where the pointer is, and there this \
                                     behaves as \"Window's monitor\".",
                                )
                                .clicked()
                            {
                                self.config.quake.display = terminale_config::QuakeDisplay::Pointer;
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
                     \u{201c}Window's monitor\u{201d} = the toggle always shows the \
                     window back on the monitor it was last visible on; drag the \
                     window to another monitor to anchor it there. \
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

        // ── Reopen in Quake on restart (general; not edge-specific) ──
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Reopen in Quake mode");
                let on = self.config.quake.restore_visible;
                if toggle_switch(ui, on).clicked() {
                    self.config.quake.restore_visible = !on;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Quake, "Reopen in Quake mode");
            sublabel(
                ui,
                "If the app is closed while the Quake drop-down is showing, reopen in \
                 Quake mode on the same monitor at next launch. Requires session restore \
                 (Workspaces → Restore last session).",
            );
        });

        ui.add_space(6.0);

        // ── Show on all virtual desktops ──
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Show on all desktops");
                let on = self.config.quake.show_on_all_desktops;
                if toggle_switch(ui, on).clicked() {
                    self.config.quake.show_on_all_desktops = !on;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Quake, "Show on all desktops");
            sublabel(
                ui,
                "Keep the Quake window on every virtual desktop / workspace, so it stays \
                 on screen when you switch desktop — no need to hide and re-show it. \
                 Works on Windows, macOS and Linux/X11. A native Wayland surface has no \
                 protocol for it, so there the window appears on whichever workspace the \
                 hotkey is pressed (see Desktop integration › Windowing backend).",
            );
        });

        ui.add_space(6.0);

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
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Easing");
                egui::ComboBox::from_id_salt("quake_easing_combo")
                    .selected_text(self.config.quake.easing.label())
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for e in terminale_config::QuakeEasing::all() {
                            if ui
                                .selectable_value(&mut self.config.quake.easing, e, e.label())
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Quake, "Easing");
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Frame rate");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.quake.animation_fps, 15..=240)
                        .suffix(" fps")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Quake, "Frame rate");
            sublabel(
                ui,
                "Slide and Bounce reveal the window from the dock edge, with the docked edge \
                 pinned. Bounce adds a spring overshoot. Scale zooms from a point at the edge. \
                 Fade animates the window's opacity instead of its geometry. None is instant.\n\
                 Easing shapes the curve: Mirror plays a close as the open in reverse, which is \
                 what stops a close from collapsing at once and then creeping the last few pixels.\n\
                 Frame rate caps how often the animation repaints. It is a ceiling, not a target — \
                 the animation is paced by the event loop, so without a cap the resize events it \
                 generates make it repaint hundreds of times a second and the compositor falls \
                 behind, which looks slower rather than smoother. Raise it on a high-refresh \
                 display; lower it to spend less GPU per toggle.",
            );
        });

        if dirty {
            self.dirty = true;
        }
    }
}
