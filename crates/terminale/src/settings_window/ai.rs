// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_ai(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "AI assistant",
            "Pick the default provider and configure each backend.",
        );

        let mut dirty = false;
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Default provider");
                egui::ComboBox::from_id_salt("ai_default_provider")
                    .selected_text(ai_provider_label(&self.config.ai.default_provider))
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        for opt in ["claude", "openai", "ollama"] {
                            if ui
                                .selectable_value(
                                    &mut self.config.ai.default_provider,
                                    opt.to_string(),
                                    ai_provider_label(opt),
                                )
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Ai, "Default provider");
            sublabel(
                ui,
                "Which backend the assistant invokes by default when no `--provider` flag is passed.",
            );
        });

        ui.add_space(6.0);

        // Markdown rendering toggle.
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Render markdown");
                if toggle_switch(ui, self.config.ai.render_markdown).clicked() {
                    self.config.ai.render_markdown = !self.config.ai.render_markdown;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Ai, "Render markdown");
            sublabel(
                ui,
                "Format AI replies — code blocks, bold, italic, lists. Off shows raw text.",
            );
        });

        ui.add_space(6.0);

        // Offer fix on failure toggle.
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Offer fix on failure");
                if toggle_switch(ui, self.config.ai.offer_fix_on_failure).clicked() {
                    self.config.ai.offer_fix_on_failure = !self.config.ai.offer_fix_on_failure;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Ai, "Offer fix on failure");
            sublabel(
                ui,
                "Show a hint after a command fails (non-zero exit) reminding you that \
                 \"Fix last command\" is available. Never sends data automatically.",
            );
        });

        ui.add_space(6.0);

        // ── Command suggestions ──
        section_subheader(ui, "Command suggestions");

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Command suggestions");
                if toggle_switch(ui, self.config.ai.suggestions.enabled).clicked() {
                    self.config.ai.suggestions.enabled = !self.config.ai.suggestions.enabled;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Ai, "Command suggestions");
            sublabel(
                ui,
                "Reads recent terminal output and proposes the next command in a \
                 bar at the bottom of the window, with a button to drop it onto \
                 the prompt for review.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            use terminale_config::SuggestionTrigger;
            let hr = ui.horizontal(|ui| {
                field_label(ui, "When to suggest");
                egui::ComboBox::from_id_salt("ai_suggestion_trigger")
                    .selected_text(self.config.ai.suggestions.trigger.label())
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for trigger in SuggestionTrigger::all() {
                            if ui
                                .selectable_value(
                                    &mut self.config.ai.suggestions.trigger,
                                    trigger,
                                    trigger.label(),
                                )
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Ai, "When to suggest");
            sublabel(
                ui,
                "Off, Manual (on a keypress / palette action), or Automatic when \
                 the terminal is idle at a prompt.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Idle delay");
                if ui
                    .add(
                        egui::Slider::new(&mut self.config.ai.suggestions.idle_secs, 1..=60)
                            .suffix(" s")
                            .text(""),
                    )
                    .changed()
                {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Ai, "Idle delay");
            sublabel(
                ui,
                "Automatic mode: seconds of no output before a suggestion fires.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Context lines");
                if ui
                    .add(
                        egui::DragValue::new(&mut self.config.ai.suggestions.context_lines)
                            .range(10..=2000)
                            .speed(1.0),
                    )
                    .changed()
                {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Ai, "Context lines");
            sublabel(
                ui,
                "How many trailing output lines are sent to the model as context.",
            );
        });

        ui.add_space(6.0);

        // Claude.
        card(ui, |ui| {
            ui.label(egui::RichText::new("Claude (Anthropic)").strong());
            let hr = ui.horizontal(|ui| {
                field_label(ui, "API key");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.config.ai.claude.api_key)
                        .password(true)
                        .desired_width(360.0)
                        .hint_text("$ANTHROPIC_API_KEY if empty"),
                );
                if r.changed() {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Ai, "API key");
            sublabel(
                ui,
                "Stored in the OS keychain — never written to config.toml.",
            );
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Model");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.config.ai.claude.model)
                            .desired_width(280.0),
                    )
                    .changed()
                {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Ai, "Model");
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Max tokens");
                if ui
                    .add(
                        egui::DragValue::new(&mut self.config.ai.claude.max_tokens)
                            .range(64..=200_000)
                            .speed(64.0),
                    )
                    .changed()
                {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Ai, "Max tokens");
        });

        ui.add_space(6.0);

        // OpenAI.
        card(ui, |ui| {
            ui.label(egui::RichText::new("OpenAI").strong());
            let hr = ui.horizontal(|ui| {
                field_label(ui, "API key");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.config.ai.openai.api_key)
                            .password(true)
                            .desired_width(360.0)
                            .hint_text("$OPENAI_API_KEY if empty"),
                    )
                    .changed()
                {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Ai, "API key");
            sublabel(
                ui,
                "Stored in the OS keychain — never written to config.toml.",
            );
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Model");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.config.ai.openai.model)
                            .desired_width(280.0),
                    )
                    .changed()
                {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Ai, "Model");
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Max tokens");
                if ui
                    .add(
                        egui::DragValue::new(&mut self.config.ai.openai.max_tokens)
                            .range(64..=200_000)
                            .speed(64.0),
                    )
                    .changed()
                {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Ai, "Max tokens");
        });

        ui.add_space(6.0);

        // Ollama.
        card(ui, |ui| {
            ui.label(egui::RichText::new("Ollama (local)").strong());
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Endpoint");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.config.ai.ollama.url)
                            .desired_width(360.0),
                    )
                    .changed()
                {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Ai, "Endpoint");
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Model");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.config.ai.ollama.model)
                            .desired_width(280.0),
                    )
                    .changed()
                {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Ai, "Model");
        });

        if dirty {
            self.dirty = true;
        }
    }
}
