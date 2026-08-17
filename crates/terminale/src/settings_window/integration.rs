// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_integration(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Desktop integration",
            "How terminale registers itself with the desktop (application menu, search, shortcuts).",
        );

        #[cfg(target_os = "linux")]
        {
            let mut dirty = false;

            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Register application-menu entry");
                    let on = self.config.integration.desktop_entry;
                    if toggle_switch(ui, on).clicked() {
                        let now_on = !on;
                        self.config.integration.desktop_entry = now_on;
                        dirty = true;
                        // Apply immediately so the launcher entry appears or
                        // disappears without waiting for the next launch.
                        if now_on {
                            let _ = crate::desktop_entry::ensure_installed();
                        } else {
                            crate::desktop_entry::remove();
                        }
                    }
                    ui.add_space(8.0);
                    let on = self.config.integration.desktop_entry;
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
                    Section::Integration,
                    "Register application-menu entry",
                );
                sublabel(
                    ui,
                    "Writes a freedesktop .desktop entry and icon under \
                     ~/.local/share so terminale shows up in the application menu and \
                     launcher search. Idempotent and refreshed automatically when the \
                     executable moves. Disable to keep terminale CLI-only.",
                );
            });

            ui.add_space(6.0);

            // ── Windowing backend ────────────────────────────────────────────
            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Windowing backend");
                    let current = self.config.integration.linux_backend;
                    egui::ComboBox::from_id_salt("linux_backend")
                        .selected_text(current.label())
                        .show_ui(ui, |ui| {
                            for backend in terminale_config::LinuxBackend::all() {
                                if ui
                                    .selectable_label(current == backend, backend.label())
                                    .clicked()
                                {
                                    self.config.integration.linux_backend = backend;
                                    dirty = true;
                                }
                            }
                        });
                });
                self.highlight_row(
                    ui,
                    hr.response.rect,
                    Section::Integration,
                    "Windowing backend",
                );
                sublabel(
                    ui,
                    "Wayland does not let an application place its own windows, so on a \
                     native Wayland surface Quake edge docking, the Snap actions, the \
                     startup position, cursor-anchored menus and tab tear-out all do \
                     nothing. \"Auto\" therefore runs through X11/XWayland whenever an X \
                     server is available. Pick \"Wayland\" only if you prefer a native \
                     surface and can live without window positioning. Takes effect on \
                     the next launch.",
                );
            });

            ui.add_space(6.0);

            // ── Quake hotkey under Wayland ───────────────────────────────────
            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Global shortcut via desktop portal");
                    let on = self.config.integration.global_shortcuts_portal;
                    if toggle_switch(ui, on).clicked() {
                        self.config.integration.global_shortcuts_portal = !on;
                        dirty = true;
                    }
                    ui.add_space(8.0);
                    let on = self.config.integration.global_shortcuts_portal;
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
                    Section::Integration,
                    "Global shortcut via desktop portal",
                );
                sublabel(
                    ui,
                    "Under Wayland no application may grab keys globally, so the Quake \
                     hotkey never fires. This registers it with the desktop instead \
                     (org.freedesktop.portal.GlobalShortcuts — GNOME 48+, KDE Plasma 6+): \
                     the desktop asks you to confirm the binding once, then delivers it \
                     whatever has focus. From then on the shortcut is re-bound in your \
                     desktop's own keyboard settings, not here. Ignored on X11, where the \
                     normal key grab works. Takes effect on the next launch.",
                );
            });

            ui.add_space(6.0);

            // ── Control socket ───────────────────────────────────────────────
            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Control socket");
                    let on = self.config.integration.control_socket;
                    if toggle_switch(ui, on).clicked() {
                        self.config.integration.control_socket = !on;
                        dirty = true;
                    }
                    ui.add_space(8.0);
                    let on = self.config.integration.control_socket;
                    ui.label(
                        egui::RichText::new(if on { "Enabled" } else { "Disabled" }).color(if on {
                            egui::Color32::from_rgb(120, 220, 140)
                        } else {
                            egui::Color32::from_rgb(140, 150, 175)
                        }),
                    );
                });
                self.highlight_row(ui, hr.response.rect, Section::Integration, "Control socket");
                sublabel(
                    ui,
                    "Lets a second terminale invocation drive this one. Its one use today \
                     is `terminale --toggle-quake`, which shows or hides the drop-down — \
                     bind that command to a key in GNOME Settings, KDE, sway, i3 or \
                     Hyprland and you get a working Quake hotkey anywhere, including \
                     compositors with no global-shortcuts portal. The socket lives in \
                     $XDG_RUNTIME_DIR and is owner-only. Takes effect on the next launch.",
                );
                if self.config.integration.control_socket {
                    ui.horizontal(|ui| {
                        ui.add_space(2.0);
                        if ui
                            .button("Copy `terminale --toggle-quake`")
                            .on_hover_text("Copy the command to bind to a key in your desktop")
                            .clicked()
                        {
                            ui.ctx()
                                .copy_text(crate::desktop_shortcut::toggle_command());
                        }
                    });
                }
            });

            ui.add_space(6.0);

            // ── One-click GNOME keybinding ───────────────────────────────────
            self.section_gnome_quake_shortcut(ui);

            if dirty {
                self.dirty = true;
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            card(ui, |ui| {
                sublabel(
                    ui,
                    "On this platform desktop integration is handled at install time: \
                     the Windows MSI registers Start-Menu and Desktop shortcuts, and the \
                     macOS app bundle is placed in /Applications. Nothing to configure here.",
                );
            });
        }
    }

    /// One-click registration of a GNOME custom keybinding that runs
    /// `terminale --toggle-quake`.
    ///
    /// This is the escape hatch for the case the global-shortcuts portal can't
    /// cover: the portal identifies callers by application id, which a process
    /// only has when the desktop launched it from its `.desktop` entry. A
    /// custom keybinding has no such requirement, so it works however terminale
    /// was started — at the cost of one explicit click, which is right for
    /// something that writes to the user's desktop settings.
    #[cfg(target_os = "linux")]
    pub(super) fn section_gnome_quake_shortcut(&mut self, ui: &mut egui::Ui) {
        use crate::desktop_shortcut;

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Quake shortcut in GNOME");
                let bound = desktop_shortcut::gnome_current_binding();
                let (text, colour) = match &bound {
                    Some(accel) => (
                        format!("Bound to {accel}"),
                        egui::Color32::from_rgb(120, 220, 140),
                    ),
                    None => (
                        "Not registered".to_string(),
                        egui::Color32::from_rgb(140, 150, 175),
                    ),
                };
                ui.label(egui::RichText::new(text).color(colour));
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Integration,
                "Quake shortcut in GNOME",
            );
            sublabel(
                ui,
                "Registers the key from Shortcuts › Quake toggle as a GNOME custom \
                 keybinding that runs `terminale --toggle-quake`. This is the way to get \
                 a working Quake hotkey under Wayland when the global-shortcuts portal \
                 is unavailable — it needs no application id, so it works however \
                 terminale was launched. Only terminale's own entry is touched; your \
                 other custom shortcuts are left alone.",
            );

            if !desktop_shortcut::gnome_available() {
                sublabel(
                    ui,
                    "GNOME keyboard settings were not found on this system (no `gsettings`, \
                     or a non-GNOME desktop). Bind the copied command by hand in your \
                     desktop's keyboard settings instead.",
                );
                return;
            }

            let binding = self.config.keybinds.quake.clone();
            ui.horizontal(|ui| {
                ui.add_space(2.0);
                let can_register = !binding.trim().is_empty();
                if ui
                    .add_enabled(can_register, egui::Button::new("Register in GNOME"))
                    .on_hover_text(format!("Bind {binding} to the Quake toggle"))
                    .clicked()
                {
                    self.quake_shortcut_status =
                        Some(match desktop_shortcut::register_gnome(&binding) {
                            Ok(accel) => format!("Registered — press {accel} to toggle."),
                            Err(e) => format!("Could not register: {e}"),
                        });
                }
                if ui.button("Remove").clicked() {
                    self.quake_shortcut_status = Some(match desktop_shortcut::unregister_gnome() {
                        Ok(()) => "Removed the GNOME shortcut.".to_string(),
                        Err(e) => format!("Could not remove: {e}"),
                    });
                }
            });
            if let Some(status) = self.quake_shortcut_status.clone() {
                sublabel(ui, &status);
            }
        });
    }
}
