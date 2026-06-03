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
             Enabling/disabling the host or changing the folder requires a \
             restart; the permission toggles below apply live.",
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

        // ── Permissions (applied live) ──
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Allow scrollback read");
                if toggle_switch(ui, self.config.plugins.allow_scrollback_read).clicked() {
                    self.config.plugins.allow_scrollback_read =
                        !self.config.plugins.allow_scrollback_read;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Plugins,
                "Allow scrollback read",
            );
            sublabel(
                ui,
                "Lets plugins read terminal contents (get_scrollback / get_visible_text). \
                 Off by default: terminal output can contain secrets. Applies live.",
            );
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Scrollback read cap");
                let r = ui.add(
                    egui::DragValue::new(&mut self.config.plugins.scrollback_read_cap)
                        .range(0..=terminale_config::plugins::SCROLLBACK_READ_CAP_MAX)
                        .speed(100)
                        .suffix(" lines"),
                );
                if r.changed() {
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Plugins,
                "Scrollback read cap",
            );
            sublabel(
                ui,
                "Maximum lines a plugin can read per call (bounds the copy). Applies live.",
            );
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Allow plugin keybindings");
                if toggle_switch(ui, self.config.plugins.allow_keybindings).clicked() {
                    self.config.plugins.allow_keybindings = !self.config.plugins.allow_keybindings;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Plugins,
                "Allow plugin keybindings",
            );
            sublabel(
                ui,
                "Lets plugins register shortcuts via register_keybinding. Plugin bindings \
                 can never shadow your own keybinds or shortcuts. Applies live.",
            );
        });

        // Show the list of currently-loaded plugins (read-only; runtime info).
        let names = self.loaded_plugin_names.clone();
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Loaded plugins");
                if names.is_empty() {
                    ui.label(egui::RichText::new("none").color(ui.visuals().weak_text_color()));
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
                sublabel(
                    ui,
                    "Plugins are loaded at startup from the directory above.",
                );
            }
        });

        if dirty {
            self.dirty = true;
        }
    }
}
