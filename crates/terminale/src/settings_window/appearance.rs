// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_appearance(&mut self, ui: &mut egui::Ui) {
        page_header(ui, "Appearance", "Colour theme and tab sizing.");

        // Refresh the cached theme list (only does disk work when stale). The
        // body below runs every frame and egui repaints continuously while the
        // scroll area has momentum, so it must not touch the disk.
        self.ensure_theme_cache();

        let mut dirty = false;

        // ── Theme picker ──
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Theme");
                let mut chosen: Option<String> = None;
                let current = self.config.appearance.theme.clone();
                egui::ComboBox::from_id_salt("theme_combo")
                    .selected_text(current.clone())
                    .width(280.0)
                    .show_ui(ui, |ui| {
                        for t in &self.cached_all_themes {
                            if ui.selectable_label(t.name == current, &t.name).clicked() {
                                chosen = Some(t.name.clone());
                            }
                        }
                    });
                if let Some(name) = chosen {
                    if name != self.config.appearance.theme {
                        self.config.appearance.theme = name;
                        dirty = true;
                    }
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Appearance, "Theme");
            sublabel(
                ui,
                "Background, cursor, selection and 16-color ANSI palette.",
            );

            // Mini swatches preview. Resolve from the cached list rather than
            // `self.config.appearance.resolved()`, which re-scans the themes
            // directory from disk on every call.
            ui.add_space(8.0);
            let resolved = self
                .cached_all_themes
                .iter()
                .find(|t| t.name == self.config.appearance.theme)
                .map_or_else(
                    || terminale_config::builtin_themes()[0].resolved(),
                    terminale_config::Theme::resolved,
                );
            ui.horizontal(|ui| {
                color_swatch(ui, resolved.background, "BG");
                color_swatch(ui, resolved.foreground, "FG");
                color_swatch(ui, resolved.cursor, "Cursor");
                color_swatch(ui, resolved.selection, "Sel");
                ui.add_space(8.0);
                for (i, c) in resolved.normal.iter().enumerate() {
                    color_swatch(ui, *c, &format!("{i}"));
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(180.0); // align under the bright row
                for (i, c) in resolved.bright.iter().enumerate() {
                    color_swatch(ui, *c, &format!("b{i}"));
                }
            });
        });

        // ── Theme import ──
        {
            ui.add_space(6.0);
            section_subheader(ui, "Theme import");
            ui.add_space(4.0);

            card(ui, |ui| {
                // Themes directory path
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Themes directory");
                    let current_dir = self
                        .config
                        .appearance
                        .themes_dir
                        .as_deref()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let mut dir_edit = current_dir.clone();
                    let hint = terminale_config::paths::themes_dir().map_or_else(
                        || "<config dir>/themes".to_owned(),
                        |p| p.to_string_lossy().into_owned(),
                    );
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut dir_edit)
                            .hint_text(hint)
                            .desired_width(240.0),
                    );
                    if r.changed() {
                        use std::path::PathBuf;
                        self.config.appearance.themes_dir = if dir_edit.is_empty() {
                            None
                        } else {
                            Some(PathBuf::from(&dir_edit))
                        };
                        dirty = true;
                        self.theme_cache_dirty = true;
                    }
                    if ui.button("\u{1f4c2}").on_hover_text("Browse…").clicked() {
                        if let Some(picked) = rfd::FileDialog::new().pick_folder() {
                            self.config.appearance.themes_dir = Some(picked);
                            dirty = true;
                            self.theme_cache_dirty = true;
                        }
                    }
                    if self.config.appearance.themes_dir.is_some()
                        && ui.small_button("Reset").clicked()
                    {
                        self.config.appearance.themes_dir = None;
                        dirty = true;
                        self.theme_cache_dirty = true;
                    }
                });
                self.highlight_row(
                    ui,
                    hr.response.rect,
                    Section::Appearance,
                    "Themes directory",
                );
                sublabel(
                    ui,
                    "Directory scanned at startup for *.toml theme files. Empty = use the default (config_dir/themes). Set to an empty path to disable directory scanning.",
                );

                ui.add_space(6.0);

                // Import button
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Import theme");
                    if ui.button("Import theme\u{2026}").clicked() {
                        self.pending_import_theme = true;
                    }
                });
                self.highlight_row(ui, hr.response.rect, Section::Appearance, "Import theme");
                sublabel(
                    ui,
                    "Open a file picker to choose a .toml theme file. The file is copied into the themes directory so it persists across launches, then selected as the active theme.",
                );

                // Drop-in themes list. Served from the cache (rebuilt on import
                // or themes-dir change) instead of re-scanning the directory
                // from disk on every frame.
                if !self.cached_dropin_names.is_empty() {
                    ui.add_space(6.0);
                    sublabel(
                        ui,
                        &format!(
                            "Drop-in themes loaded from directory ({}):",
                            self.cached_dropin_names.len()
                        ),
                    );
                    for name in &self.cached_dropin_names {
                        ui.label(format!("  \u{2022} {name}"));
                    }
                }
            });
        }

        // ── Background image ──
        {
            use terminale_config::BgImageFit;

            ui.add_space(6.0);
            section_subheader(ui, "Background image");
            ui.add_space(4.0);

            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Image path");
                    let path_str = self
                        .config
                        .appearance
                        .background_image
                        .path
                        .clone()
                        .unwrap_or_default();
                    let mut path_edit = path_str.clone();
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut path_edit)
                            .hint_text("Path to PNG / JPEG / WebP / GIF…")
                            .desired_width(260.0),
                    );
                    if r.changed() {
                        self.config.appearance.background_image.path = if path_edit.is_empty() {
                            None
                        } else {
                            Some(path_edit)
                        };
                        dirty = true;
                    }
                    if ui.button("\u{1f4c2}").on_hover_text("Browse…").clicked() {
                        if let Some(picked) = rfd::FileDialog::new()
                            .add_filter(
                                "Images",
                                &[
                                    "png", "jpg", "jpeg", "webp", "gif", "PNG", "JPG", "JPEG",
                                    "WEBP", "GIF",
                                ],
                            )
                            .pick_file()
                        {
                            self.config.appearance.background_image.path =
                                Some(picked.to_string_lossy().into_owned());
                            dirty = true;
                        }
                    }
                    if self.config.appearance.background_image.path.is_some()
                        && ui.small_button("Clear").clicked()
                    {
                        self.config.appearance.background_image.path = None;
                        dirty = true;
                    }
                });
                self.highlight_row(ui, hr.response.rect, Section::Appearance, "Image path");
                sublabel(
                    ui,
                    "Absolute path to an image file drawn behind the terminal text.",
                );
            });
            ui.add_space(6.0);

            let img_has_path = self.config.appearance.background_image.path.is_some();

            card(ui, |ui| {
                ui.add_enabled_ui(img_has_path, |ui| {
                    let hr = ui.horizontal(|ui| {
                        field_label(ui, "Image fit");
                        egui::ComboBox::from_id_salt("bg_image_fit_combo")
                            .selected_text(self.config.appearance.background_image.fit.label())
                            .width(160.0)
                            .show_ui(ui, |ui| {
                                for fit in BgImageFit::all() {
                                    if ui
                                        .selectable_value(
                                            &mut self.config.appearance.background_image.fit,
                                            fit,
                                            fit.label(),
                                        )
                                        .clicked()
                                    {
                                        dirty = true;
                                    }
                                }
                            });
                    });
                    self.highlight_row(ui, hr.response.rect, Section::Appearance, "Image fit");
                    sublabel(ui, "How the image is scaled to fill the window.");
                    ui.add_space(4.0);

                    let hr = ui.horizontal(|ui| {
                        field_label(ui, "Image opacity");
                        let r = ui.add(
                            egui::Slider::new(
                                &mut self.config.appearance.background_image.opacity,
                                0.0..=1.0,
                            )
                            .fixed_decimals(2)
                            .text(""),
                        );
                        if r.changed() {
                            dirty = true;
                        }
                    });
                    self.highlight_row(ui, hr.response.rect, Section::Appearance, "Image opacity");
                    sublabel(ui, "0.0 = fully transparent, 1.0 = fully opaque.");
                    ui.add_space(4.0);

                    let hr = ui.horizontal(|ui| {
                        field_label(ui, "Image brightness");
                        let r = ui.add(
                            egui::Slider::new(
                                &mut self.config.appearance.background_image.brightness,
                                0.0..=2.0,
                            )
                            .fixed_decimals(2)
                            .text(""),
                        );
                        if r.changed() {
                            dirty = true;
                        }
                    });
                    self.highlight_row(
                        ui,
                        hr.response.rect,
                        Section::Appearance,
                        "Image brightness",
                    );
                    sublabel(ui, "Multiplier. 1.0 = unchanged; 0.5 = half brightness.");
                    ui.add_space(4.0);

                    let hr = ui.horizontal(|ui| {
                        field_label(ui, "Image saturation");
                        let r = ui.add(
                            egui::Slider::new(
                                &mut self.config.appearance.background_image.saturation,
                                0.0..=2.0,
                            )
                            .fixed_decimals(2)
                            .text(""),
                        );
                        if r.changed() {
                            dirty = true;
                        }
                    });
                    self.highlight_row(
                        ui,
                        hr.response.rect,
                        Section::Appearance,
                        "Image saturation",
                    );
                    sublabel(ui, "Multiplier. 0.0 = greyscale, 1.0 = unchanged.");
                    ui.add_space(4.0);

                    let hr = ui.horizontal(|ui| {
                        field_label(ui, "Image hue");
                        let r = ui.add(
                            egui::Slider::new(
                                &mut self.config.appearance.background_image.hue,
                                0.0..=360.0,
                            )
                            .suffix("\u{00b0}")
                            .fixed_decimals(0)
                            .text(""),
                        );
                        if r.changed() {
                            dirty = true;
                        }
                    });
                    self.highlight_row(ui, hr.response.rect, Section::Appearance, "Image hue");
                    sublabel(ui, "Hue rotation in degrees. 0 = unchanged.");
                });
            });
        }

        // ── Background FX (animated wallpaper, any theme) ──
        {
            use terminale_config::BackgroundFxStyle;

            ui.add_space(6.0);
            section_subheader(ui, "Background FX");
            ui.add_space(4.0);

            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Enable background FX");
                    let on = self.config.background_fx.enabled;
                    if toggle_switch(ui, on).clicked() {
                        self.config.background_fx.enabled = !on;
                        dirty = true;
                    }
                });
                self.highlight_row(
                    ui,
                    hr.response.rect,
                    Section::Appearance,
                    "Enable background FX",
                );
                sublabel(
                    ui,
                    "Animated wallpaper drawn behind the terminal text — aurora, starfield, matrix rain, or pixel-CRT.",
                );
            });
            ui.add_space(6.0);

            let bg_on = self.config.background_fx.enabled;

            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Background style");
                    egui::ComboBox::from_id_salt("bg_fx_style_combo")
                        .selected_text(self.config.background_fx.style.label())
                        .width(200.0)
                        .show_ui(ui, |ui| {
                            for style in BackgroundFxStyle::all() {
                                let r = ui.add_enabled(
                                    bg_on,
                                    egui::SelectableLabel::new(
                                        self.config.background_fx.style == style,
                                        style.label(),
                                    ),
                                );
                                if r.clicked() {
                                    self.config.background_fx.style = style;
                                    dirty = true;
                                }
                            }
                        });
                });
                self.highlight_row(
                    ui,
                    hr.response.rect,
                    Section::Appearance,
                    "Background style",
                );
                sublabel(
                    ui,
                    "Keep intensity modest so text stays readable. None keeps your selection while the toggle is off.",
                );
            });
            ui.add_space(6.0);

            card(ui, |ui| {
                ui.add_enabled_ui(bg_on, |ui| {
                    let hr = ui.horizontal(|ui| {
                        field_label(ui, "Background intensity");
                        let r = ui.add(
                            egui::Slider::new(&mut self.config.background_fx.intensity, 0.0..=1.0)
                                .fixed_decimals(2)
                                .text(""),
                        );
                        if r.changed() {
                            dirty = true;
                        }
                    });
                    self.highlight_row(
                        ui,
                        hr.response.rect,
                        Section::Appearance,
                        "Background intensity",
                    );
                    sublabel(
                        ui,
                        "Opacity / strength of the effect. 0.35 = default (subtle).",
                    );
                    ui.add_space(4.0);

                    let hr = ui.horizontal(|ui| {
                        field_label(ui, "Background speed");
                        let r = ui.add(
                            egui::Slider::new(&mut self.config.background_fx.speed, 0.1..=5.0)
                                .fixed_decimals(2)
                                .text(""),
                        );
                        if r.changed() {
                            dirty = true;
                        }
                    });
                    self.highlight_row(
                        ui,
                        hr.response.rect,
                        Section::Appearance,
                        "Background speed",
                    );
                    sublabel(ui, "Animation speed multiplier. 1.0 = default.");
                    ui.add_space(4.0);

                    let hr = ui.horizontal(|ui| {
                        field_label(ui, "React to keystrokes");
                        let on = self.config.background_fx.react_to_keystrokes;
                        if toggle_switch(ui, on).clicked() {
                            self.config.background_fx.react_to_keystrokes = !on;
                            dirty = true;
                        }
                    });
                    self.highlight_row(
                        ui,
                        hr.response.rect,
                        Section::Appearance,
                        "React to keystrokes",
                    );
                    sublabel(
                        ui,
                        "Each keypress spawns an independent animated band that travels and decays — multiple keystrokes layer.",
                    );
                    ui.add_space(4.0);

                    let hr = ui.horizontal(|ui| {
                        field_label(ui, "Pause when unfocused");
                        let on = self.config.background_fx.pause_when_unfocused;
                        if toggle_switch(ui, on).clicked() {
                            self.config.background_fx.pause_when_unfocused = !on;
                            dirty = true;
                        }
                    });
                    self.highlight_row(
                        ui,
                        hr.response.rect,
                        Section::Appearance,
                        "Pause when unfocused",
                    );
                    sublabel(
                        ui,
                        "Stop animating the background while the window is not focused, so a background terminal costs no GPU. On by default. (A minimized or fully-covered window never animates regardless.)",
                    );
                    ui.add_space(4.0);

                    let hr = ui.horizontal(|ui| {
                        field_label(ui, "Band lifetime");
                        let r = ui.add(
                            egui::Slider::new(
                                &mut self.config.background_fx.band_lifetime_secs,
                                0.5..=8.0,
                            )
                            .fixed_decimals(1)
                            .suffix(" s")
                            .text(""),
                        );
                        if r.changed() {
                            dirty = true;
                        }
                    });
                    self.highlight_row(
                        ui,
                        hr.response.rect,
                        Section::Appearance,
                        "Band lifetime",
                    );
                    sublabel(ui, "How long each keystroke band lives before fully fading. 2.5 s = default.");
                    ui.add_space(4.0);

                    let hr = ui.horizontal(|ui| {
                        field_label(ui, "Matrix band width");
                        let r = ui.add(
                            egui::Slider::new(
                                &mut self.config.background_fx.matrix_band_width,
                                1_u32..=8,
                            )
                            .suffix(" cols")
                            .text(""),
                        );
                        if r.changed() {
                            dirty = true;
                        }
                    });
                    self.highlight_row(
                        ui,
                        hr.response.rect,
                        Section::Appearance,
                        "Matrix band width",
                    );
                    sublabel(ui, "Width of each Matrix rain band in character columns. 3 = default.");
                    ui.add_space(4.0);

                    let hr = ui.horizontal(|ui| {
                        field_label(ui, "Matrix fall speed");
                        let r = ui.add(
                            egui::Slider::new(
                                &mut self.config.background_fx.matrix_fall_speed,
                                4.0..=60.0,
                            )
                            .fixed_decimals(1)
                            .suffix(" rows/s")
                            .text(""),
                        );
                        if r.changed() {
                            dirty = true;
                        }
                    });
                    self.highlight_row(
                        ui,
                        hr.response.rect,
                        Section::Appearance,
                        "Matrix fall speed",
                    );
                    sublabel(ui, "Base fall speed for Matrix rain bands. 14.0 = default.");
                    ui.add_space(4.0);

                    let hr = ui.horizontal(|ui| {
                        field_label(ui, "Max concurrent bands");
                        let r = ui.add(
                            egui::Slider::new(
                                &mut self.config.background_fx.max_emitters,
                                1_u32..=64,
                            )
                            .text(""),
                        );
                        if r.changed() {
                            dirty = true;
                        }
                    });
                    self.highlight_row(
                        ui,
                        hr.response.rect,
                        Section::Appearance,
                        "Max concurrent bands",
                    );
                    sublabel(ui, "Maximum number of overlapping animated bands at once. 48 = default.");
                    ui.add_space(4.0);

                    // Optional custom tints — `None` (auto) uses per-style defaults.
                    let hr = ui.horizontal(|ui| {
                        field_label(ui, "Custom colors");
                        let mut custom = self.config.background_fx.color1.is_some()
                            || self.config.background_fx.color2.is_some();
                        if toggle_switch(ui, custom).clicked() {
                            custom = !custom;
                            if custom {
                                self.config.background_fx.color1 = Some([140, 80, 210]);
                                self.config.background_fx.color2 = Some([40, 180, 200]);
                            } else {
                                self.config.background_fx.color1 = None;
                                self.config.background_fx.color2 = None;
                            }
                            dirty = true;
                        }
                        if let Some(c) = self.config.background_fx.color1.as_mut() {
                            if ui.color_edit_button_srgb(c).changed() {
                                dirty = true;
                            }
                        }
                        if let Some(c) = self.config.background_fx.color2.as_mut() {
                            if ui.color_edit_button_srgb(c).changed() {
                                dirty = true;
                            }
                        }
                    });
                    self.highlight_row(ui, hr.response.rect, Section::Appearance, "Custom colors");
                    sublabel(ui, "Off = each style uses its own palette.");
                });
            });
        }

        // ── Faint/dim intensity ───────────────────────────────────────────────
        {
            ui.add_space(6.0);
            section_subheader(ui, "Text rendering");
            ui.add_space(4.0);

            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Faint/dim intensity");
                    let r = ui.add(
                        egui::Slider::new(&mut self.config.appearance.dim_amount, 0.0..=1.0)
                            .fixed_decimals(2)
                            .text(""),
                    );
                    if r.changed() {
                        dirty = true;
                    }
                });
                self.highlight_row(
                    ui,
                    hr.response.rect,
                    Section::Appearance,
                    "Faint/dim intensity",
                );
                sublabel(
                    ui,
                    "How strongly SGR 2 (faint) text is blended toward the background. 0.0 = no dimming, 1.0 = invisible, 0.5 = default.",
                );
            });

            ui.add_space(6.0);

            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Minimum contrast");
                    let r = ui.add(
                        egui::Slider::new(&mut self.config.appearance.minimum_contrast, 1.0..=21.0)
                            .fixed_decimals(1)
                            .text(""),
                    );
                    if r.changed() {
                        dirty = true;
                    }
                    if ui.small_button("Reset").clicked() {
                        self.config.appearance.minimum_contrast = 1.0;
                        dirty = true;
                    }
                });
                self.highlight_row(
                    ui,
                    hr.response.rect,
                    Section::Appearance,
                    "Minimum contrast",
                );
                sublabel(
                    ui,
                    "Minimum WCAG contrast ratio between text and its background. 1.0 = disabled (default). 4.5 = WCAG AA; 7.0 = WCAG AAA. Low-contrast text is nudged lighter or darker to meet the threshold.",
                );
            });

            ui.add_space(6.0);

            card(ui, |ui| {
                let hr = ui.horizontal(|ui| {
                    field_label(ui, "Builtin box drawing");
                    let on = self.config.appearance.builtin_box_drawing;
                    if toggle_switch(ui, on).clicked() {
                        self.config.appearance.builtin_box_drawing = !on;
                        dirty = true;
                    }
                });
                self.highlight_row(
                    ui,
                    hr.response.rect,
                    Section::Appearance,
                    "Builtin box drawing",
                );
                sublabel(
                    ui,
                    "Render box-drawing (U+2500\u{2013}U+257F) and block-element (U+2580\u{2013}U+259F) characters as crisp procedural quads aligned to the cell grid. Eliminates font-glyph seams in TUI boxes and progress bars. On by default.",
                );
            });
        }

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Tab min width");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.appearance.tab_min_width, 40.0..=400.0)
                        .step_by(1.0)
                        .suffix(" px")
                        .text(""),
                );
                if r.changed() {
                    // Keep the upper bound at or above the lower bound.
                    if self.config.appearance.tab_max_width < self.config.appearance.tab_min_width {
                        self.config.appearance.tab_max_width = self.config.appearance.tab_min_width;
                    }
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Appearance, "Tab min width");
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Tab max width");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.appearance.tab_max_width, 40.0..=400.0)
                        .step_by(1.0)
                        .suffix(" px")
                        .text(""),
                );
                if r.changed() {
                    if self.config.appearance.tab_min_width > self.config.appearance.tab_max_width {
                        self.config.appearance.tab_min_width = self.config.appearance.tab_max_width;
                    }
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Appearance, "Tab max width");
            ui.horizontal(|ui| {
                if ui.small_button("Reset").clicked() {
                    self.config.appearance.tab_min_width = 90.0;
                    self.config.appearance.tab_max_width = 260.0;
                    dirty = true;
                }
            });
            sublabel(
                ui,
                "Bounds a tab clamps to. Tabs shrink toward the min as you open more, and grow toward the max for long titles.",
            );

            ui.add_space(4.0);

            let hr = ui.horizontal(|ui| {
                field_label(ui, "Pinned tab width");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.appearance.pinned_tab_width, 24.0..=120.0)
                        .step_by(1.0)
                        .suffix(" px")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Appearance,
                "Pinned tab width",
            );
            sublabel(
                ui,
                "Fixed width of a pinned (compact, icon-only) tab chip. Pinned tabs always sit at the front of the bar.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            if ui
                .checkbox(
                    &mut self.config.appearance.animated_tab_drag,
                    "Animated tab drag",
                )
                .changed()
            {
                dirty = true;
            }
            sublabel(
                ui,
                "Chrome-style drag: a ghost tab follows the cursor with a drop indicator, and a torn-out window appears only on release. Off = a plainer drag that still reorders / attaches / tears out on release.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                let r = ui.checkbox(
                    &mut self.config.appearance.show_pane_headers,
                    "Show pane headers",
                );
                if r.changed() {
                    dirty = true;
                }
                r
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Appearance,
                "Show pane headers",
            );
            sublabel(
                ui,
                "Show a 22 px title strip with a close \u{2716} above each pane when a tab has more than one pane. Off = reclaim the vertical space.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                let r = ui.checkbox(
                    &mut self.config.appearance.tab_activity_spinner,
                    "Activity spinner on busy tabs",
                );
                if r.changed() {
                    dirty = true;
                }
                r
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Appearance,
                "Activity spinner on busy tabs",
            );
            sublabel(
                ui,
                "Prepend a braille-dots animated spinner to the tab label (and to each pane header in split view) while a command is running or output is flowing. Off = no spinner.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                let r = ui.checkbox(&mut self.config.appearance.pane_tear_out, "Tear out panes");
                if r.changed() {
                    dirty = true;
                }
                r
            });
            self.highlight_row(ui, hr.response.rect, Section::Appearance, "Tear out panes");
            sublabel(
                ui,
                "Drag a pane header to detach it into its own tab (drop on a tab bar) or a new window (drop outside). Requires pane headers to be visible.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Close button style");
                egui::ComboBox::from_id_salt("close_button_style_combo")
                    .selected_text(self.config.appearance.close_button_style.label())
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for style in terminale_config::CloseButtonStyle::all() {
                            if ui
                                .selectable_value(
                                    &mut self.config.appearance.close_button_style,
                                    style,
                                    style.label(),
                                )
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Appearance,
                "Close button style",
            );
            sublabel(
                ui,
                "Chip: a small filled square behind the X strokes. Bare: only the X strokes, no background.",
            );
        });

        ui.add_space(12.0);
        section_subheader(ui, "Tab bar");
        ui.add_space(4.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Tab bar enabled");
                let on = self.config.appearance.tab_bar_enabled;
                if toggle_switch(ui, on).clicked() {
                    self.config.appearance.tab_bar_enabled = !on;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Appearance, "Tab bar enabled");
            sublabel(ui, "Show or hide the tab bar completely. When hidden, all space is reclaimed for the terminal grid.");
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Tab bar position");
                let current = self.config.appearance.tab_bar_position;
                egui::ComboBox::from_id_salt("tab_bar_position_combo")
                    .selected_text(current.label())
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for pos in terminale_config::TabBarPosition::all() {
                            if ui
                                .selectable_value(
                                    &mut self.config.appearance.tab_bar_position,
                                    pos,
                                    pos.label(),
                                )
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Appearance,
                "Tab bar position",
            );
            sublabel(
                ui,
                "Top / Bottom: horizontal bar. Left / Right: vertical strip on the side.",
            );

            // Width slider — only shown when the position is Left or Right.
            if self.config.appearance.tab_bar_position.is_vertical() {
                ui.add_space(4.0);
                let hr2 = ui.horizontal(|ui| {
                    field_label(ui, "Vertical tab bar width");
                    let r = ui.add(
                        egui::Slider::new(
                            &mut self.config.appearance.vertical_tab_bar_width,
                            120.0..=360.0,
                        )
                        .step_by(1.0)
                        .suffix(" px")
                        .text(""),
                    );
                    if r.changed() {
                        dirty = true;
                    }
                    if ui.small_button("Reset").clicked() {
                        self.config.appearance.vertical_tab_bar_width = 180.0;
                        dirty = true;
                    }
                });
                self.highlight_row(
                    ui,
                    hr2.response.rect,
                    Section::Appearance,
                    "Vertical tab bar width",
                );
                sublabel(ui, "Width of the side strip in logical pixels (120–360).");
            }
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Hide tab bar if single tab");
                let on = self.config.appearance.tab_bar_hide_if_single;
                if toggle_switch(ui, on).clicked() {
                    self.config.appearance.tab_bar_hide_if_single = !on;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Appearance,
                "Hide tab bar if single tab",
            );
            sublabel(
                ui,
                "Automatically hide the tab bar when only one tab is open; show it again as soon as a second tab appears.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Show tab group labels");
                let on = self.config.appearance.show_tab_group_labels;
                if toggle_switch(ui, on).clicked() {
                    self.config.appearance.show_tab_group_labels = !on;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Appearance,
                "Show tab group labels",
            );
            sublabel(
                ui,
                "Show a Chrome-style coloured pill with the group name in the gap to the left of each grouped run of tabs. The colour accent spine is always visible regardless of this setting.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            ui.label(egui::RichText::new("Group colours").strong());
            ui.add_space(4.0);
            sublabel(
                ui,
                "Auto-cycle palette used when creating new tab groups. Each row is one colour; new groups cycle through the list. Must have at least one entry.",
            );
            ui.add_space(4.0);
            let mut remove_idx: Option<usize> = None;
            for (i, rgb) in self
                .config
                .appearance
                .tab_group_colors
                .iter_mut()
                .enumerate()
            {
                ui.horizontal(|ui| {
                    ui.label(format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]));
                    if ui.color_edit_button_srgb(rgb).changed() {
                        dirty = true;
                    }
                    if ui.small_button("Remove").clicked() {
                        remove_idx = Some(i);
                    }
                });
            }
            if let Some(idx) = remove_idx {
                // Keep at least one colour so validation always passes.
                if self.config.appearance.tab_group_colors.len() > 1 {
                    self.config.appearance.tab_group_colors.remove(idx);
                    dirty = true;
                }
            }
            if ui.small_button("Add colour").clicked() {
                self.config
                    .appearance
                    .tab_group_colors
                    .push([0x7d, 0xa6, 0xff]);
                dirty = true;
            }
        });

        ui.add_space(12.0);
        section_subheader(ui, "Split panes");
        ui.add_space(4.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Divider thickness");
                let r = ui.add(
                    egui::Slider::new(
                        &mut self.config.appearance.divider_thickness_logical,
                        1.0..=12.0,
                    )
                    .suffix(" px")
                    .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.appearance.divider_thickness_logical = 4.0;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Appearance,
                "Divider thickness",
            );
            sublabel(
                ui,
                "Visible stroke width of the line drawn between split panes, in logical pixels.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Divider grab padding");
                let r = ui.add(
                    egui::Slider::new(
                        &mut self.config.appearance.divider_grab_padding_logical,
                        0.0..=20.0,
                    )
                    .suffix(" px")
                    .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.appearance.divider_grab_padding_logical = 3.0;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Appearance,
                "Divider grab padding",
            );
            sublabel(
                ui,
                "Extra logical pixels on each side of the divider stroke that still start a drag. Higher = easier to grab.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Divider colour");
                let auto = self.config.appearance.divider_color.is_none();
                let mut use_auto = auto;
                if ui.checkbox(&mut use_auto, "Auto").changed() {
                    if use_auto {
                        self.config.appearance.divider_color = None;
                    } else {
                        // Default to a neutral grey when the user first
                        // enables a custom colour.
                        self.config.appearance.divider_color = Some([80, 90, 110]);
                    }
                    dirty = true;
                }
                if let Some(ref mut rgb) = self.config.appearance.divider_color {
                    if ui.color_edit_button_srgb(rgb).changed() {
                        dirty = true;
                    }
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Appearance, "Divider colour");
            sublabel(
                ui,
                "Auto = renderer picks a neutral tone derived from the background. Custom = pick an exact colour.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Focus border thickness");
                let r = ui.add(
                    egui::Slider::new(
                        &mut self.config.appearance.focus_border_thickness_logical,
                        0.0..=8.0,
                    )
                    .suffix(" px")
                    .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.appearance.focus_border_thickness_logical = 2.0;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Appearance,
                "Focus border thickness",
            );
            sublabel(
                ui,
                "Stroke width of the accent ring drawn around the focused split pane. Set to 0 to disable.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Focus border colour");
                let auto = self.config.appearance.focus_border_color.is_none();
                let mut use_auto = auto;
                if ui.checkbox(&mut use_auto, "Auto").changed() {
                    if use_auto {
                        self.config.appearance.focus_border_color = None;
                    } else {
                        self.config.appearance.focus_border_color = Some([125, 166, 255]);
                    }
                    dirty = true;
                }
                if let Some(ref mut rgb) = self.config.appearance.focus_border_color {
                    if ui.color_edit_button_srgb(rgb).changed() {
                        dirty = true;
                    }
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Appearance,
                "Focus border colour",
            );
            sublabel(
                ui,
                "Auto = built-in blue accent [0x7d, 0xa6, 0xff]. Custom = pick an exact colour.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Dim inactive panes");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.appearance.inactive_pane_dim, 0.0..=0.9)
                        .fixed_decimals(2)
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.appearance.inactive_pane_dim = 0.0;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Appearance,
                "Dim inactive panes",
            );
            sublabel(
                ui,
                "Alpha of a black overlay drawn over non-focused panes in a split tab. 0.0 = off (default); 0.4 = noticeable; 0.9 = strong.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Dim unfocused window");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.appearance.unfocused_window_dim, 0.0..=0.9)
                        .fixed_decimals(2)
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.appearance.unfocused_window_dim = 0.0;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Appearance,
                "Dim unfocused window",
            );
            sublabel(
                ui,
                "Alpha of a black overlay drawn over the whole grid when the window loses OS focus. 0.0 = off (default); 0.3 = subtle; 0.9 = strong.",
            );
        });

        // ── Icon set ──────────────────────────────────────────────────────────
        ui.add_space(6.0);
        section_subheader(ui, "Icons");
        ui.add_space(4.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Use bundled icon set");
                let r = ui.add(egui::Checkbox::without_text(
                    &mut self.config.appearance.bundled_icons,
                ));
                if r.changed() {
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Appearance,
                "Use bundled icon set",
            );
            sublabel(
                ui,
                "Clean line icons (recommended). Off = classic emoji icons.",
            );
        });

        ui.add_space(6.0);
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Resource indicators");
                let on = self.config.resource_indicators.enabled;
                if toggle_switch(ui, on).clicked() {
                    self.config.resource_indicators.enabled = !on;
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
                Section::Appearance,
                "Resource indicators",
            );
            sublabel(
                ui,
                "Pixel-art CPU / RAM meters plus the GPU name, in a strip at the bottom of the \
                 window. The grid shrinks slightly to make room, so it never overlaps content.",
            );
        });

        if dirty {
            self.dirty = true;
        }
    }
}
