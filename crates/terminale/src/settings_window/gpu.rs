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
             All settings on this page require a restart to take effect.",
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

        if dirty {
            self.dirty = true;
        }
    }
}