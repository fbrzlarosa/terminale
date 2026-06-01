// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_plugins(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Plugins",
            "Lua 5.4 scripts loaded at startup from your plugins folder. \
             All settings on this page require a restart to take effect.",
        );

        let mut dirty = false;
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Enabled");
                if toggle_switch(ui, self.config.plugins.enabled).clicked() {
                    self.config.plugins.enabled = !self.config.plugins.enabled;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Plugins, "Enabled");
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Directory");
                let mut path_str = self
                    .config
                    .plugins
                    .directory
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                let r = ui.add(
                    egui::TextEdit::singleline(&mut path_str)
                        .desired_width(360.0)
                        .hint_text("(default location)"),
                );
                if r.changed() {
                    self.config.plugins.directory = if path_str.trim().is_empty() {
                        None
                    } else {
                        Some(std::path::PathBuf::from(path_str))
                    };
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Plugins, "Directory");
            sublabel(
                ui,
                "Default: ~/.config/terminale/plugins (or the OS equivalent). (requires restart)",
            );
        });

        // Show the list of currently-loaded plugins (read-only; runtime info).
        let names = self.loaded_plugin_names.clone();
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Loaded plugins");
                if names.is_empty() {
                    ui.label(
                        egui::RichText::new("none")
                            .color(ui.visuals().weak_text_color()),
                    );
                } else {
                    ui.label(format!("{} plugin(s)", names.len()));
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Plugins, "Loaded plugins");
            if !names.is_empty() {
                ui.add_space(4.0);
                for name in &names {
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        ui.label(
                            egui::RichText::new(name)
                                .monospace()
                                .color(ui.visuals().strong_text_color()),
                        );
                    });
                }
                sublabel(ui, "Plugins are loaded at startup from the directory above.");
            }
        });

        if dirty {
            self.dirty = true;
        }
    }
}