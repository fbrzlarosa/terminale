// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_gpu(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "GPU",
            "Pick the graphics backend or disable hardware acceleration. \
             Backend and power preference require a restart; presentation \
             applies immediately.",
        );

        let mut dirty = false;

        // Backend picker.
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Backend");
                egui::ComboBox::from_id_salt("gpu_backend_combo")
                    .selected_text(self.config.gpu.backend.label())
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for b in terminale_config::GpuBackend::all() {
                            if ui
                                .selectable_value(&mut self.config.gpu.backend, b, b.label())
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Gpu, "Backend");
            sublabel(
                ui,
                "Auto lets the renderer choose. Force Vulkan / Direct3D 12 / Metal / OpenGL for a \
                 specific API. Software disables the GPU and renders on the CPU (slow, but a useful \
                 fallback on broken drivers). (requires restart)",
            );
        });

        ui.add_space(6.0);

        // Power-preference picker.
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Power");
                egui::ComboBox::from_id_salt("gpu_power_combo")
                    .selected_text(self.config.gpu.power_preference.label())
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for p in terminale_config::GpuPowerPreference::all() {
                            if ui
                                .selectable_value(
                                    &mut self.config.gpu.power_preference,
                                    p,
                                    p.label(),
                                )
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Gpu, "Power");
            sublabel(
                ui,
                "Auto leaves the choice to the driver. Low power favours an integrated GPU; high \
                 performance favours a discrete GPU. Ignored when Backend is Software. \
                 (requires restart)",
            );
        });

        ui.add_space(6.0);

        // Presentation (vsync) picker — applied live to every window.
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Presentation");
                egui::ComboBox::from_id_salt("gpu_present_mode_combo")
                    .selected_text(self.config.gpu.present_mode.label())
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        for m in terminale_config::GpuPresentMode::all() {
                            if ui
                                .selectable_value(&mut self.config.gpu.present_mode, m, m.label())
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Gpu, "Presentation");
            sublabel(
                ui,
                "Auto never makes a window wait for the display, and paces redraws to the \
                 monitor's refresh rate: with several windows open, one waiting on vsync would \
                 hold up all the others. Vsync always waits — choose it only if you see tearing \
                 (an X11 desktop without a compositor).",
            );
        });

        if dirty {
            self.dirty = true;
        }
    }
}
