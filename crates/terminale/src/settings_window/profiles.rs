// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_profiles(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Profiles",
            "Choose which shell terminale launches and add your own.",
        );

        let mut remove_idx: Option<usize> = None;
        let mut new_default: Option<String> = None;
        let mut duplicate_idx: Option<usize> = None;

        // Default profile select — quick switcher at the top.
        let current_default = self.config.profiles.default.clone();
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Default profile");
                egui::ComboBox::from_id_salt("default_profile_combo")
                    .selected_text(current_default.clone().unwrap_or_else(|| "—".to_string()))
                    .width(280.0)
                    .show_ui(ui, |ui| {
                        for p in &self.config.profiles.profiles {
                            if ui
                                .selectable_label(
                                    current_default.as_deref() == Some(p.name.as_str()),
                                    &p.name,
                                )
                                .clicked()
                            {
                                new_default = Some(p.name.clone());
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Profiles, "Default profile");
            sublabel(
                ui,
                "Used when no `--profile` flag is passed on the command line.",
            );
        });

        ui.add_space(10.0);

        // Per-profile cards.
        for (idx, profile) in self.config.profiles.profiles.iter_mut().enumerate() {
            let is_default = current_default.as_deref() == Some(profile.name.as_str());
            profile_card(
                ui,
                idx,
                profile,
                is_default,
                &self.detected_shells,
                &mut self.dirty,
                || remove_idx = Some(idx),
                || duplicate_idx = Some(idx),
            );
            ui.add_space(6.0);
        }

        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("  ➕  New profile  ").color(egui::Color32::WHITE),
                    )
                    .fill(egui::Color32::from_rgb(45, 60, 110))
                    .rounding(0.0)
                    .min_size(egui::vec2(0.0, 32.0)),
                )
                .clicked()
            {
                self.add_blank_profile();
            }
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("  🔍  Re-scan shells  ").color(egui::Color32::WHITE),
                    )
                    .fill(egui::Color32::from_rgb(40, 45, 60))
                    .rounding(0.0)
                    .min_size(egui::vec2(0.0, 32.0)),
                )
                .clicked()
            {
                self.detected_shells = auto_detect_profiles();
                self.status = Some((
                    StatusKind::Success,
                    format!("Detected {} shells.", self.detected_shells.len()),
                ));
            }
        });

        if let Some(idx) = duplicate_idx {
            let copy = self.config.profiles.profiles[idx].clone();
            self.config.profiles.profiles.insert(
                idx + 1,
                Profile {
                    name: format!("{} (copy)", copy.name),
                    ..copy
                },
            );
            self.dirty = true;
        }
        if let Some(idx) = remove_idx {
            let removed = self.config.profiles.profiles.remove(idx);
            if self.config.profiles.default.as_deref() == Some(removed.name.as_str()) {
                self.config.profiles.default = self
                    .config
                    .profiles
                    .profiles
                    .first()
                    .map(|p| p.name.clone());
            }
            self.dirty = true;
        }
        if let Some(name) = new_default {
            self.config.profiles.default = Some(name);
            self.dirty = true;
        }
    }
}