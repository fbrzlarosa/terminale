// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_clipboard_history(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Clipboard History",
            "In-memory ring of recent copy actions — re-paste any retained entry via the picker.",
        );

        let mut dirty = false;

        // ── Enable / disable ─────────────────────────────────────────────────

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Enable clipboard history");
                let on = self.config.clipboard_history.enabled;
                if toggle_switch(ui, on).clicked() {
                    self.config.clipboard_history.enabled = !on;
                    dirty = true;
                }
                ui.add_space(8.0);
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
                Section::ClipboardHistory,
                "Enable clipboard history",
            );
            sublabel(
                ui,
                "When enabled, each copy action adds the text to a memory-only ring. \
                 Nothing is written to disk.",
            );
        });

        ui.add_space(6.0);

        // ── Ring size ─────────────────────────────────────────────────────────

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "History size");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.clipboard_history.size, 1_usize..=500_usize)
                        .suffix(" entries")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.clipboard_history.size = 20;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::ClipboardHistory,
                "History size",
            );
            sublabel(
                ui,
                "Maximum number of entries the ring keeps. Oldest are evicted when the ring is full.",
            );
        });

        ui.add_space(6.0);

        // ── OSC 52 capture ────────────────────────────────────────────────────

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Capture OSC 52 writes");
                let on = self.config.clipboard_history.capture_osc52;
                if toggle_switch(ui, on).clicked() {
                    self.config.clipboard_history.capture_osc52 = !on;
                    dirty = true;
                }
                ui.add_space(8.0);
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
                Section::ClipboardHistory,
                "Capture OSC 52 writes",
            );
            sublabel(
                ui,
                "When enabled, text written to the clipboard by running applications (OSC 52) \
                 is also captured. Disabled by default — OSC 52 payloads often contain \
                 tokens or secrets that should not appear in the history picker.",
            );
        });

        if dirty {
            self.dirty = true;
        }
    }
}
