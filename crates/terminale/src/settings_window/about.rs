// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_about(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "About", "");

        card(ui, |ui| {
            ui.label(
                egui::RichText::new("terminale")
                    .heading()
                    .color(egui::Color32::from_rgb(220, 230, 255)),
            );
            ui.label(
                egui::RichText::new(env!("CARGO_PKG_VERSION"))
                    .small()
                    .color(egui::Color32::from_rgb(120, 130, 160)),
            );
            ui.add_space(8.0);
            ui.label("A native, cross-platform, GPU-accelerated terminal.");
            ui.add_space(6.0);
            ui.hyperlink_to(
                "https://stackbyte.dev/terminale",
                "https://stackbyte.dev/terminale",
            );
            ui.add_space(4.0);
            ui.hyperlink_to(
                "github.com/fbrzlarosa/terminale",
                "https://github.com/fbrzlarosa/terminale",
            );
        });

        ui.add_space(10.0);
        card(ui, |ui| {
            let pre_rect = ui.min_rect();
            ui.label(
                egui::RichText::new("Config file")
                    .strong()
                    .color(egui::Color32::from_rgb(220, 230, 255)),
            );
            let label_rect = ui.min_rect().union(pre_rect);
            self.highlight_row(ui, label_rect, Section::About, "Config file");
            ui.label(
                egui::RichText::new(self.config_path.display().to_string())
                    .monospace()
                    .small()
                    .color(egui::Color32::from_rgb(150, 160, 190)),
            );
            ui.add_space(8.0);
            if ui.button("Open in file manager").clicked() {
                if let Some(parent) = self.config_path.parent() {
                    let _ = open::that_detached(parent);
                }
            }
        });
    }
}
