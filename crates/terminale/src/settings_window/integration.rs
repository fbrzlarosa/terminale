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
}
