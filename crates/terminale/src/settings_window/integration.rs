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

            // ── Control API scopes ───────────────────────────────────────────
            self.section_control_api(ui, &mut dirty);

            ui.add_space(6.0);

            // ── One-click GNOME keybinding ───────────────────────────────────
            self.section_gnome_quake_shortcut(ui);

            ui.add_space(6.0);

            // ── Cold-start behaviour of the desktop-owned hotkey ─────────────
            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Start terminale on the Quake hotkey");
                    let on = self.config.integration.quake_launch_on_demand;
                    if toggle_switch(ui, on).clicked() {
                        self.config.integration.quake_launch_on_demand = !on;
                        dirty = true;
                    }
                    ui.add_space(8.0);
                    let on = self.config.integration.quake_launch_on_demand;
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
                    "Start terminale on the Quake hotkey",
                );
                sublabel(
                    ui,
                    "A desktop keybinding runs `terminale --toggle-quake`, which talks to a \
                     running terminale — so on the first press after logging in there was nothing \
                     to talk to and the key did nothing at all. With this on, that first press \
                     starts terminale instead, and every press after it toggles. Only a missing \
                     socket triggers it: an instance that is running but wedged is not answered \
                     by starting a second one. Applies to the next press.",
                );
            });

            ui.add_space(6.0);

            // ── Warm start ──────────────────────────────────────────────────
            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Start hidden when you log in");
                    let on = self.config.integration.autostart;
                    if toggle_switch(ui, on).clicked() {
                        let now_on = !on;
                        self.config.integration.autostart = now_on;
                        dirty = true;
                        // Apply immediately, in both directions: a switch that
                        // only ever writes the entry is a switch that cannot be
                        // turned off.
                        if now_on {
                            if let Err(e) = crate::desktop_entry::ensure_autostart() {
                                self.quake_extension_status =
                                    Some(format!("Could not write the autostart entry: {e}"));
                            }
                        } else {
                            crate::desktop_entry::remove_autostart();
                        }
                    }
                    ui.add_space(8.0);
                    let on = self.config.integration.autostart;
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
                    "Start hidden when you log in",
                );
                sublabel(
                    ui,
                    "This is what makes the drop-down feel instant. A hotkey that has to                      start terminale cannot be quick however fast terminale is — the                      process, the GPU surface and the shell all have to come up before                      anything appears. With this on all of that happens at login, the                      window simply stays unmapped, and the first press of the hotkey is a                      reveal like every press after it. Writes an autostart entry under                      ~/.config/autostart; turning it off removes it.",
                );
            });

            ui.add_space(6.0);

            // ── Hand the drop-down to a shell extension ─────────────────────
            self.section_quake_extension(ui, &mut dirty);

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

    /// What `terminale ctl` (and anything else on the control socket) may do.
    ///
    /// Four switches rather than one, because the interesting question is not
    /// "is automation on" but "may it read my scrollback" and "may it press
    /// Enter". Submitting is off by default and stays visibly off here, since
    /// that is the difference between a tool that drafts a command for you and
    /// one that runs it.
    #[cfg(target_os = "linux")]
    pub(super) fn section_control_api(&mut self, ui: &mut egui::Ui, dirty: &mut bool) {
        // Nothing here can do anything without the socket, so mirror that.
        let socket_on = self.config.integration.control_socket;

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Automation & AI control");
                let on = self.config.integration.control_api.enabled;
                ui.add_enabled_ui(socket_on, |ui| {
                    if toggle_switch(ui, on).clicked() {
                        self.config.integration.control_api.enabled = !on;
                        *dirty = true;
                    }
                });
                ui.add_space(8.0);
                let on = self.config.integration.control_api.enabled && socket_on;
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
                "Automation & AI control",
            );
            sublabel(
                ui,
                "Serve the `terminale ctl` commands on the control socket: list tabs and \
                 panes, read a pane, fetch the last command with its exit code, run any \
                 command-palette action, type at a prompt, take a screenshot. This is what \
                 makes terminale scriptable — and what lets an AI coding agent see what \
                 your last command printed instead of guessing. Applies immediately. \
                 With this off, only the Quake toggle is served.",
            );

            if !socket_on {
                sublabel(
                    ui,
                    "Turn on the control socket above first — it is the channel these \
                     commands arrive on.",
                );
                return;
            }

            let api_on = self.config.integration.control_api.enabled;
            ui.add_space(4.0);
            ui.add_enabled_ui(api_on, |ui| {
                let mut scope = |label: &str, value: &mut bool, help: &str| {
                    let hr = ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        field_label(ui, label);
                        if toggle_switch(ui, *value).clicked() {
                            *value = !*value;
                            *dirty = true;
                        }
                    });
                    let _ = hr;
                    sublabel(ui, help);
                };

                scope(
                    "May read terminal content",
                    &mut self.config.integration.control_api.allow_read,
                    "Allows get-text, last-command, and the titles and working directories \
                     in list-tabs. Scrollback holds whatever your commands printed, tokens \
                     included — turn this off if you would rather nothing could read it.",
                );
                scope(
                    "May type into a shell",
                    &mut self.config.integration.control_api.allow_input,
                    "Allows send-text, send-keys and running palette actions. Text lands at \
                     the prompt for you to read; it is not run.",
                );
                scope(
                    "May press Enter (run commands)",
                    &mut self.config.integration.control_api.allow_submit,
                    "Off by default, and the one switch worth thinking about: with it on, \
                     anything that can reach the socket can execute commands as you. Leave \
                     it off and an automation tool can only compose a command for you to \
                     confirm. Turn it on for scripted or CI use.",
                );
                scope(
                    "May take screenshots",
                    &mut self.config.integration.control_api.allow_screenshot,
                    "Allows the screenshot command to render the window to a PNG at a path \
                     the caller chooses. Separate from reading text because an image leaks \
                     the same content in a form you cannot grep.",
                );
                scope(
                    "Serve MCP to AI agents",
                    &mut self.config.integration.mcp.enabled,
                    "Lets an AI agent connect to `terminale mcp` and call the commands above \
                     as MCP tools — so it can read what your last command printed and what it \
                     exited with instead of asking you to paste it. Register it with \
                     `claude mcp add terminale -- terminale mcp`. It grants nothing the \
                     switches above do not: with \"May press Enter\" off, an agent can only \
                     compose a command for you to confirm.",
                );
            });
        });
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

    /// Hand the drop-down over to a shell extension that implements one.
    ///
    /// On GNOME under Wayland an application cannot own a global key, cannot
    /// place its own window and cannot animate it onto the screen — the three
    /// things a drop-down terminal is made of. A shell extension can do all
    /// three, which is why a terminal with a working drop-down on this desktop is
    /// in fact being driven by one. Terminale can either fight that or join it;
    /// this card joins it.
    ///
    /// Two halves, in the order they have to happen: install a launcher entry
    /// with an identity of its own, then point the extension at that entry.
    #[cfg(target_os = "linux")]
    pub(super) fn section_quake_extension(&mut self, ui: &mut egui::Ui, dirty: &mut bool) {
        use crate::{desktop_entry, desktop_shortcut};

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Drop-down via shell extension");
                let on = self.config.integration.quake_desktop_entry;
                if toggle_switch(ui, on).clicked() {
                    let now_on = !on;
                    self.config.integration.quake_desktop_entry = now_on;
                    *dirty = true;
                    // Apply immediately: the extension's app picker reads the
                    // application list, so the entry has to be there before the
                    // user goes looking for it.
                    if now_on {
                        if let Err(e) = desktop_entry::ensure_quake_launcher() {
                            self.quake_extension_status =
                                Some(format!("Could not write the launcher entry: {e}"));
                        }
                    } else {
                        desktop_entry::remove_quake_launcher();
                    }
                }
                ui.add_space(8.0);
                let on = self.config.integration.quake_desktop_entry;
                ui.label(
                    egui::RichText::new(if on { "Installed" } else { "Not installed" }).color(
                        if on {
                            egui::Color32::from_rgb(120, 220, 140)
                        } else {
                            egui::Color32::from_rgb(140, 150, 175)
                        },
                    ),
                );
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Integration,
                "Drop-down via shell extension",
            );
            sublabel(
                ui,
                "Installs a second launcher entry, terminale.Quake.desktop, whose only job is to \
                 be the app a drop-down shell extension launches and toggles. It carries an \
                 application id of its own so the extension drives that window and never the \
                 terminale you were working in, and it does not turn on terminale's own drop-down \
                 docking — when an extension owns the geometry and the animation, doing both is \
                 what makes the drop-down look like it is fighting the desktop. It works from \
                 a fresh login because the extension starts the app itself.",
            );

            if !self.config.integration.quake_desktop_entry {
                return;
            }

            ui.add_space(4.0);
            let entry_id = desktop_entry::quake_launcher_id();

            if !desktop_shortcut::quake_extension_available() {
                sublabel(
                    ui,
                    "No drop-down extension was found. Install \"Quake Terminal\" from \
                     extensions.gnome.org, then set its Application to the entry below. Any \
                     extension of that kind works — they all launch an app by its desktop-entry \
                     id.",
                );
                ui.horizontal(|ui| {
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(entry_id)
                            .monospace()
                            .color(egui::Color32::from_rgb(200, 210, 230)),
                    );
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(entry_id.to_string());
                    }
                });
                return;
            }

            let current = desktop_shortcut::quake_extension_app();
            let ours = current.as_deref() == Some(entry_id);
            ui.horizontal(|ui| {
                ui.add_space(2.0);
                let (text, colour) = if ours {
                    (
                        "The extension is driving terminale".to_string(),
                        egui::Color32::from_rgb(120, 220, 140),
                    )
                } else {
                    (
                        match &current {
                            Some(app) => format!("The extension is driving {app}"),
                            None => "The extension has no application set".to_string(),
                        },
                        egui::Color32::from_rgb(220, 190, 120),
                    )
                };
                ui.label(egui::RichText::new(text).color(colour));
            });
            if let Some(key) = desktop_shortcut::quake_extension_shortcut() {
                sublabel(
                    ui,
                    &format!(
                        "Its key is {key}, and it belongs to the extension — which is also why \
                         recording that same combination here does nothing: the desktop swallows \
                         it before terminale can see it. Leave it with the extension and there is \
                         nothing to bind in terminale at all."
                    ),
                );
            }
            ui.horizontal(|ui| {
                ui.add_space(2.0);
                if ui
                    .add_enabled(!ours, egui::Button::new("Point it at terminale"))
                    .on_hover_text(format!(
                        "Set the extension's application to {entry_id}, keeping its key, size and \
                         animation as you have them"
                    ))
                    .clicked()
                {
                    self.quake_extension_status =
                        Some(match desktop_shortcut::point_quake_extension_at(entry_id) {
                            Ok(key) => format!(
                                "Done — {key} now opens terminale. The first press starts it."
                            ),
                            Err(e) => format!("Could not update the extension: {e}"),
                        });
                }
                if ui.button("Copy entry id").clicked() {
                    ui.ctx().copy_text(entry_id.to_string());
                }
            });
            if let Some(status) = self.quake_extension_status.clone() {
                sublabel(ui, &status);
            }
        });
    }
}
