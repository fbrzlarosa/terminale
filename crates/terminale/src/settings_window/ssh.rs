// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_ssh(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "SSH hosts",
            "Named remotes you can open as a tab from the command palette (\"SSH: <name>\") or the New SSH tab picker.",
        );

        let mut dirty = false;
        let mut remove_idx: Option<usize> = None;

        // ── Host-key verification ─────────────────────────────────────────────
        card(ui, |ui| {
            ui.label(
                egui::RichText::new("Host-key verification")
                    .strong()
                    .color(egui::Color32::from_rgb(210, 220, 245)),
            );
            ui.add_space(6.0);

            let hr = ui.horizontal(|ui| {
                field_label(ui, "Host key policy");
                let current = self.config.ssh.host_key_policy;
                egui::ComboBox::from_id_salt("ssh_host_key_policy")
                    .selected_text(current.label())
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for policy in terminale_config::HostKeyPolicy::all() {
                            if ui
                                .selectable_value(
                                    &mut self.config.ssh.host_key_policy,
                                    policy,
                                    policy.label(),
                                )
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Ssh, "Host key policy");
            sublabel(
                ui,
                "Accept new: pin on first connect (TOFU), refuse changed keys. \
                 Strict: refuse any host not already known. \
                 Off: accept any key (disables MITM detection).",
            );

            ui.add_space(6.0);

            let hr2 = ui.horizontal(|ui| {
                field_label(ui, "known_hosts file");
                let mut path_str = self.config.ssh.known_hosts.display().to_string();
                let r = ui.add(
                    egui::TextEdit::singleline(&mut path_str)
                        .desired_width(340.0)
                        .hint_text("~/.ssh/known_hosts")
                        .font(egui::TextStyle::Monospace),
                );
                if r.changed() {
                    self.config.ssh.known_hosts = std::path::PathBuf::from(path_str);
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr2.response.rect, Section::Ssh, "known_hosts file");
            sublabel(
                ui,
                "Path to the SSH known-hosts file. Defaults to ~/.ssh/known_hosts.",
            );
        });
        ui.add_space(6.0);

        // ── Offer to save typed SSH hosts ────────────────────────────────────
        // Whether typing an `ssh …` command for an unsaved host offers to
        // save it (the same flag the prompt's "don't ask again" box flips).
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Offer to save typed SSH hosts");
                let on = self.config.terminal.offer_save_ssh_hosts;
                if toggle_switch(ui, on).clicked() {
                    self.config.terminal.offer_save_ssh_hosts = !on;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Ssh,
                "Offer to save typed SSH hosts",
            );
            sublabel(
                ui,
                "Detect `ssh user@host` you type and offer a one-click save so it appears here.",
            );
        });
        ui.add_space(6.0);

        // ── OpenSSH config import ─────────────────────────────────────────────
        card(ui, |ui| {
            ui.label(
                egui::RichText::new("OpenSSH config import")
                    .strong()
                    .color(egui::Color32::from_rgb(210, 220, 245)),
            );
            ui.add_space(6.0);

            let hr_mode = ui.horizontal(|ui| {
                field_label(ui, "Import SSH config");
                let current = self.config.ssh.import_openssh_config;
                egui::ComboBox::from_id_salt("ssh_import_openssh_config")
                    .selected_text(current.label())
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        for mode in terminale_config::ImportOpenSshConfig::all() {
                            if ui
                                .selectable_value(
                                    &mut self.config.ssh.import_openssh_config,
                                    mode,
                                    mode.label(),
                                )
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr_mode.response.rect, Section::Ssh, "Import SSH config");
            sublabel(
                ui,
                "Off: disabled. Import once: one-shot button below. Live: merge on startup/reload (not written to config).",
            );

            ui.add_space(4.0);

            let hr_path = ui.horizontal(|ui| {
                field_label(ui, "SSH config path");
                let mut path_str = self.config.ssh.openssh_config_path.display().to_string();
                let r = ui.add(
                    egui::TextEdit::singleline(&mut path_str)
                        .desired_width(340.0)
                        .hint_text("~/.ssh/config")
                        .font(egui::TextStyle::Monospace),
                );
                if r.changed() {
                    self.config.ssh.openssh_config_path = std::path::PathBuf::from(path_str);
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr_path.response.rect, Section::Ssh, "SSH config path");
            sublabel(ui, "Path to the OpenSSH client config file to import.");

            // Show the import button only when mode is ImportOnce.
            if self.config.ssh.import_openssh_config
                == terminale_config::ImportOpenSshConfig::ImportOnce
            {
                ui.add_space(6.0);
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("  Import from SSH config  ")
                                .color(egui::Color32::WHITE),
                        )
                        .fill(egui::Color32::from_rgb(45, 80, 140))
                        .rounding(0.0)
                        .min_size(egui::vec2(0.0, 28.0)),
                    )
                    .on_hover_text(
                        "Parse the SSH config file and append new hosts to the saved list",
                    )
                    .clicked()
                {
                    self.pending_import_ssh_hosts = true;
                }
            }
        });
        ui.add_space(6.0);

        // Pull the credential-editor state out of `self` so the `iter_mut`
        // loop below (which borrows `self.config`) can still read/update it.
        // Deferred keychain actions are collected and applied after the loop.
        let mut secret_edit = self.ssh_secret_edit.take();
        let mut secret_status = self.ssh_secret_status.take();
        // (host index, secret-or-None). `Some(s)` = store `s`; `None` = clear.
        let mut secret_action: Option<(usize, Option<String>)> = None;
        // Index whose stored-secret presence we want to show — computed once
        // before the loop so we don't query the keychain every frame per host.
        let secret_present: Vec<bool> = self
            .config
            .ssh_hosts
            .iter()
            .map(|h| {
                matches!(h.auth, terminale_config::SshAuthMethod::Password)
                    && terminale_config::get_secret(&h.secret_id())
                        .ok()
                        .flatten()
                        .is_some()
            })
            .collect();

        for (idx, host) in self.config.ssh_hosts.iter_mut().enumerate() {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("🔐  {}", host.endpoint()))
                            .strong()
                            .color(egui::Color32::from_rgb(210, 220, 245)),
                    );
                    // Right-aligned remove button.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("🗑")
                                        .color(egui::Color32::from_rgb(220, 130, 130)),
                                )
                                .fill(egui::Color32::from_rgb(40, 26, 30))
                                .rounding(0.0),
                            )
                            .on_hover_text("Remove this SSH host")
                            .clicked()
                        {
                            remove_idx = Some(idx);
                        }
                    });
                });

                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    field_label(ui, "Name");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut host.name)
                            .desired_width(280.0)
                            .hint_text("display name (e.g. prod-db)"),
                    );
                    if r.changed() {
                        dirty = true;
                    }
                });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    field_label(ui, "Host");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut host.host)
                            .desired_width(280.0)
                            .hint_text("hostname or IP")
                            .font(egui::TextStyle::Monospace),
                    );
                    if r.changed() {
                        dirty = true;
                    }
                });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    field_label(ui, "Port");
                    if ui
                        .add(
                            egui::DragValue::new(&mut host.port)
                                .range(1..=65535)
                                .speed(1),
                        )
                        .changed()
                    {
                        dirty = true;
                    }
                });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    field_label(ui, "User");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut host.user)
                            .desired_width(280.0)
                            .hint_text("remote username"),
                    );
                    if r.changed() {
                        dirty = true;
                    }
                });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    field_label(ui, "Auth method");
                    egui::ComboBox::from_id_salt(("ssh_auth", idx))
                        .selected_text(host.auth.label())
                        .width(200.0)
                        .show_ui(ui, |ui| {
                            for method in terminale_config::SshAuthMethod::all() {
                                if ui
                                    .selectable_value(&mut host.auth, method, method.label())
                                    .clicked()
                                {
                                    dirty = true;
                                }
                            }
                        });
                });

                // The private-key path is only meaningful for key auth.
                if host.auth == terminale_config::SshAuthMethod::Key {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        field_label(ui, "Key path");
                        let mut key_str = host
                            .key_path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default();
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut key_str)
                                .desired_width(360.0)
                                .hint_text("~/.ssh/id_ed25519 (prefer ed25519)")
                                .font(egui::TextStyle::Monospace),
                        );
                        if r.changed() {
                            host.key_path = if key_str.trim().is_empty() {
                                None
                            } else {
                                Some(PathBuf::from(key_str))
                            };
                            dirty = true;
                        }
                    });
                }

                // Credential management for password auth: store / update /
                // clear the secret in the OS keychain. The secret never touches
                // `config.toml`.
                if host.auth == terminale_config::SshAuthMethod::Password {
                    ui.add_space(8.0);
                    let stored = secret_present.get(idx).copied().unwrap_or(false);
                    let editing = matches!(&secret_edit, Some((i, _)) if *i == idx);

                    if editing {
                        ui.horizontal(|ui| {
                            field_label(ui, "Password");
                            if let Some((_, buf)) = secret_edit.as_mut() {
                                ui.add(
                                    egui::TextEdit::singleline(buf)
                                        .password(true)
                                        .desired_width(260.0)
                                        .hint_text("type password, then save"),
                                );
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("  Save to keychain  ")
                                            .color(egui::Color32::WHITE),
                                    )
                                    .fill(egui::Color32::from_rgb(60, 110, 230))
                                    .rounding(0.0),
                                )
                                .clicked()
                            {
                                if let Some((_, buf)) = secret_edit.take() {
                                    secret_action = Some((idx, Some(buf)));
                                }
                            }
                            if ui.button("Cancel").clicked() {
                                secret_edit = None;
                            }
                        });
                    } else {
                        ui.horizontal(|ui| {
                            field_label(ui, "Password");
                            if stored {
                                ui.label(
                                    egui::RichText::new("\u{2714} stored in keychain")
                                        .color(egui::Color32::from_rgb(120, 200, 140)),
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new("not stored — prompted on connect")
                                        .color(egui::Color32::from_rgb(150, 160, 185)),
                                );
                            }
                            let label = if stored { "Update" } else { "Set password" };
                            if ui.button(label).clicked() {
                                secret_edit = Some((idx, String::new()));
                            }
                            if stored && ui.button("Clear").clicked() {
                                secret_action = Some((idx, None));
                            }
                        });
                    }

                    if let Some((i, msg)) = &secret_status {
                        if *i == idx {
                            sublabel(ui, msg);
                        }
                    }
                }

                sublabel(
                    ui,
                    "Agent auth keeps key material out of this file. Passwords live in the OS keychain, never in config.toml.",
                );
            });
            ui.add_space(6.0);
        }

        if self.config.ssh_hosts.is_empty() {
            card(ui, |ui| {
                ui.label(
                    egui::RichText::new("No SSH hosts yet. Add one below to open it as a tab.")
                        .color(egui::Color32::from_rgb(140, 150, 175)),
                );
            });
            ui.add_space(6.0);
        }

        if ui
            .add(
                egui::Button::new(
                    egui::RichText::new("  ➕  Add SSH host  ").color(egui::Color32::WHITE),
                )
                .fill(egui::Color32::from_rgb(45, 60, 110))
                .rounding(0.0)
                .min_size(egui::vec2(0.0, 32.0)),
            )
            .clicked()
        {
            self.config.ssh_hosts.push(terminale_config::SshHost {
                id: terminale_config::SshHost::new_id(),
                name: String::new(),
                host: String::new(),
                port: terminale_config::default_ssh_port(),
                user: String::new(),
                auth: terminale_config::SshAuthMethod::default(),
                key_path: None,
            });
            dirty = true;
        }

        if let Some(idx) = remove_idx {
            // Best-effort: drop any keychain secret tied to this host so we
            // don't leave an orphaned credential behind.
            if let Some(host) = self.config.ssh_hosts.get(idx) {
                if let Err(e) = terminale_config::delete_secret(&host.secret_id()) {
                    tracing::warn!(?e, "could not delete ssh secret from keychain");
                }
            }
            self.config.ssh_hosts.remove(idx);
            secret_edit = None;
            dirty = true;
        }

        // Apply a deferred keychain store / clear, then surface the result.
        if let Some((idx, value)) = secret_action {
            if let Some(host) = self.config.ssh_hosts.get(idx) {
                let id = host.secret_id();
                let msg = match value {
                    Some(secret) => match terminale_config::store_secret(&id, &secret) {
                        Ok(()) => "Saved to keychain.".to_string(),
                        Err(e) => format!("Keychain error: {e}"),
                    },
                    None => match terminale_config::delete_secret(&id) {
                        Ok(()) => "Cleared from keychain.".to_string(),
                        Err(e) => format!("Keychain error: {e}"),
                    },
                };
                secret_status = Some((idx, msg));
            }
            secret_edit = None;
        }

        // Stash the (possibly-updated) editor state back onto `self`.
        self.ssh_secret_edit = secret_edit;
        self.ssh_secret_status = secret_status;

        if dirty {
            self.dirty = true;
        }
    }
}
