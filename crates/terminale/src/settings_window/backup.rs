// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_backup(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Backup",
            "Export your settings to a single encrypted file, or restore them on another machine. \
             The file is encrypted with a passphrase you choose (Argon2id + XChaCha20-Poly1305).",
        );

        // Deferred actions so we don't borrow `self.backup` while also calling
        // `&mut self` helpers.
        let mut do_export = false;
        let mut do_import = false;

        // ── Export ──
        card(ui, |ui| {
            ui.label(
                egui::RichText::new("Export")
                    .strong()
                    .size(16.0)
                    .color(egui::Color32::from_rgb(220, 230, 255)),
            );
            sublabel(
                ui,
                "Settings are always included. Credentials are included only if you tick the box below.",
            );
            ui.add_space(8.0);

            let hr = ui.horizontal(|ui| {
                field_label(ui, "Passphrase");
                ui.add(
                    egui::TextEdit::singleline(&mut self.backup.export_pass)
                        .password(true)
                        .desired_width(260.0)
                        .hint_text("choose a strong passphrase"),
                );
            });
            self.highlight_row(ui, hr.response.rect, Section::Backup, "Passphrase");
            ui.add_space(4.0);
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Confirm");
                ui.add(
                    egui::TextEdit::singleline(&mut self.backup.export_confirm)
                        .password(true)
                        .desired_width(260.0)
                        .hint_text("re-enter passphrase"),
                );
            });
            self.highlight_row(ui, hr.response.rect, Section::Backup, "Confirm");

            ui.add_space(10.0);
            ui.checkbox(
                &mut self.backup.include_credentials,
                "Include SSH credentials from the OS keychain",
            );
            if self.backup.include_credentials {
                ui.label(
                    egui::RichText::new(
                        "\u{26A0} The backup will contain your SSH passwords (encrypted with your \
                         passphrase). Keep the file and passphrase safe.",
                    )
                    .small()
                    .color(egui::Color32::from_rgb(235, 190, 110)),
                );
            } else {
                sublabel(
                    ui,
                    "Off by default: credentials stay in the OS keychain and are NOT exported.",
                );
            }

            ui.add_space(6.0);
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("  Export to file\u{2026}  ")
                            .strong()
                            .color(egui::Color32::WHITE),
                    )
                    .fill(egui::Color32::from_rgb(60, 110, 230))
                    .rounding(0.0)
                    .min_size(egui::vec2(0.0, 32.0)),
                )
                .clicked()
            {
                do_export = true;
            }
        });

        ui.add_space(10.0);

        // ── Import ──
        card(ui, |ui| {
            ui.label(
                egui::RichText::new("Import")
                    .strong()
                    .size(16.0)
                    .color(egui::Color32::from_rgb(220, 230, 255)),
            );
            sublabel(
                ui,
                "Restore settings from an encrypted backup. Any included credentials are written \
                 back into the OS keychain. This replaces your current settings.",
            );
            ui.add_space(8.0);

            let hr = ui.horizontal(|ui| {
                field_label(ui, "Passphrase");
                ui.add(
                    egui::TextEdit::singleline(&mut self.backup.import_pass)
                        .password(true)
                        .desired_width(260.0)
                        .hint_text("passphrase used at export"),
                );
            });
            self.highlight_row(ui, hr.response.rect, Section::Backup, "Passphrase");

            ui.add_space(6.0);
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("  Import from file\u{2026}  ")
                            .color(egui::Color32::from_rgb(220, 230, 255)),
                    )
                    .fill(egui::Color32::from_rgb(40, 50, 78))
                    .rounding(0.0)
                    .min_size(egui::vec2(0.0, 32.0)),
                )
                .clicked()
            {
                do_import = true;
            }
        });

        // Status line for the last backup action.
        if let Some((kind, msg)) = &self.backup.status {
            ui.add_space(6.0);
            let color = match kind {
                StatusKind::Success => egui::Color32::from_rgb(120, 220, 140),
                StatusKind::Warning => egui::Color32::from_rgb(230, 200, 110),
                StatusKind::Error => egui::Color32::from_rgb(230, 110, 110),
            };
            ui.label(egui::RichText::new(msg).color(color));
        }

        if do_export {
            self.do_backup_export();
        }
        if do_import {
            self.do_backup_import();
        }
    }

    /// Validate, encrypt, and write the backup to a user-chosen file.
    pub(super) fn do_backup_export(&mut self) {
        let pass = self.backup.export_pass.clone();
        if pass.is_empty() {
            self.backup.status = Some((StatusKind::Error, "Enter a passphrase.".into()));
            return;
        }
        if pass != self.backup.export_confirm {
            self.backup.status = Some((StatusKind::Error, "Passphrases don't match.".into()));
            return;
        }

        // Gather credentials only when explicitly opted in.
        let mut credentials = Vec::new();
        if self.backup.include_credentials {
            for host in &self.config.ssh_hosts {
                if host.auth == terminale_config::SshAuthMethod::Password {
                    if let Ok(Some(secret)) = terminale_config::get_secret(&host.secret_id()) {
                        credentials.push(terminale_config::BackupCredential {
                            secret_id: host.secret_id(),
                            secret,
                        });
                    }
                }
            }
        }

        let payload = terminale_config::BackupPayload {
            config: self.config.clone(),
            credentials,
        };
        let blob = match terminale_config::backup::encrypt(&payload, &pass) {
            Ok(b) => b,
            Err(e) => {
                self.backup.status = Some((StatusKind::Error, format!("Encryption failed: {e}")));
                return;
            }
        };

        let Some(path) = rfd::FileDialog::new()
            .set_title("Export terminale settings")
            .set_file_name("terminale-settings.tbk")
            .add_filter("terminale backup", &["tbk"])
            .save_file()
        else {
            // User cancelled the dialog — leave state untouched, no error.
            return;
        };

        match std::fs::write(&path, &blob) {
            Ok(()) => {
                let n = payload.credentials.len();
                let extra = if n > 0 {
                    format!(" ({n} credential(s) included)")
                } else {
                    String::new()
                };
                self.backup.status = Some((
                    StatusKind::Success,
                    format!("Exported to {}{extra}.", path.display()),
                ));
                // Clear the passphrases now that we're done with them.
                self.backup.export_pass.clear();
                self.backup.export_confirm.clear();
            }
            Err(e) => {
                self.backup.status =
                    Some((StatusKind::Error, format!("Could not write file: {e}")));
            }
        }
    }

    /// Pick a file, decrypt + validate it, then apply the config and write any
    /// included credentials back into the OS keychain.
    pub(super) fn do_backup_import(&mut self) {
        let pass = self.backup.import_pass.clone();
        if pass.is_empty() {
            self.backup.status = Some((StatusKind::Error, "Enter the passphrase.".into()));
            return;
        }

        let Some(path) = rfd::FileDialog::new()
            .set_title("Import terminale settings")
            .add_filter("terminale backup", &["tbk"])
            .add_filter("all files", &["*"])
            .pick_file()
        else {
            return;
        };

        let blob = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                self.backup.status = Some((StatusKind::Error, format!("Could not read file: {e}")));
                return;
            }
        };

        let payload = match terminale_config::backup::decrypt(&blob, &pass) {
            Ok(p) => p,
            Err(e) => {
                self.backup.status = Some((StatusKind::Error, e.to_string()));
                return;
            }
        };

        // Apply the restored config (already validated inside `decrypt`) and
        // persist it to disk so it survives the next launch.
        self.config = payload.config;
        // A new id may have been backfilled on hosts; mark dirty so the save
        // bar reflects the change too.
        self.dirty = true;
        if let Err(e) = self.config.write_to(&self.config_path) {
            self.backup.status =
                Some((StatusKind::Error, format!("Imported but save failed: {e}")));
            return;
        }
        self.dirty = false;

        // Repopulate the keychain with any included credentials.
        let mut restored = 0usize;
        let mut keychain_err = None;
        for cred in &payload.credentials {
            match terminale_config::store_secret(&cred.secret_id, &cred.secret) {
                Ok(()) => restored += 1,
                Err(e) => keychain_err = Some(e.to_string()),
            }
        }

        self.backup.import_pass.clear();
        let (kind, msg) = match keychain_err {
            Some(e) => (
                StatusKind::Warning,
                format!("Imported settings, but a credential couldn't be stored: {e}"),
            ),
            None if restored > 0 => (
                StatusKind::Success,
                format!("Imported settings + {restored} credential(s) into the keychain."),
            ),
            None => (StatusKind::Success, "Imported settings.".to_string()),
        };
        self.backup.status = Some((kind, msg));
    }
}