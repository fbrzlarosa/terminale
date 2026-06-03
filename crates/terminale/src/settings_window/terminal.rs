// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_terminal(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Terminal",
            "Scrolling, selection, links, and the editor opened on file references.",
        );

        let mut dirty = false;

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Scroll step");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.window.scroll_step_lines, 1..=50)
                        .suffix(" lines")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.window.scroll_step_lines = 3;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Terminal, "Scroll step");
            sublabel(
                ui,
                "Rows scrolled per mouse-wheel notch on the main screen.",
            );

            ui.add_space(4.0);

            let hr = ui.horizontal(|ui| {
                field_label(ui, "Alt-screen scroll step");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.window.alt_screen_scroll_lines, 1..=50)
                        .suffix(" lines")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.window.alt_screen_scroll_lines = 3;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Terminal,
                "Alt-screen scroll step",
            );
            sublabel(
                ui,
                "Arrow keys sent per wheel notch when an app owns the alt-screen (editor, pager, etc.).",
            );

            ui.add_space(4.0);

            let hr = ui.horizontal(|ui| {
                field_label(ui, "Trackpad pixels per row");
                let r = ui.add(
                    egui::Slider::new(
                        &mut self.config.window.touchpad_pixels_per_row,
                        1.0_f32..=128.0,
                    )
                    .suffix(" px")
                    .step_by(1.0)
                    .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.window.touchpad_pixels_per_row = 16.0;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Terminal,
                "Trackpad pixels per row",
            );
            sublabel(
                ui,
                "How many pixels of high-resolution trackpad input equal one terminal row. \
                 Lower values scroll faster; higher values require a larger gesture per row.",
            );

            ui.add_space(4.0);

            let hr = ui.horizontal(|ui| {
                field_label(ui, "Smooth scroll");
                let on = self.config.window.smooth_scroll;
                if toggle_switch(ui, on).clicked() {
                    self.config.window.smooth_scroll = !on;
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
            self.highlight_row(ui, hr.response.rect, Section::Terminal, "Smooth scroll");
            sublabel(
                ui,
                "Accumulate sub-row trackpad deltas across events so slow gestures \
                 scroll by single rows rather than losing the fraction each event.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Scrollback");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.window.scrollback_lines, 0..=1_000_000)
                        .logarithmic(true)
                        .suffix(" lines")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.window.scrollback_lines = 10_000;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Terminal, "Scrollback");
            sublabel(
                ui,
                "History lines kept per terminal (0 disables). Applies live to all tabs.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Copy on select");
                let on = self.config.window.copy_on_select;
                if toggle_switch(ui, on).clicked() {
                    self.config.window.copy_on_select = !on;
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
            self.highlight_row(ui, hr.response.rect, Section::Terminal, "Copy on select");
            sublabel(
                ui,
                "Auto-copy the selection to the clipboard the moment you finish selecting with the mouse.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Word separators");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.config.terminal.word_separators)
                        .desired_width(360.0)
                        .hint_text("characters that break a double-click word"),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.terminal.word_separators =
                        terminale_config::TerminalConfig::default().word_separators;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Terminal, "Word separators");
            sublabel(
                ui,
                "Double-click selects the run between these characters. Whitespace always breaks; keep `.`, `/`, `-`, `_` out to select paths and identifiers whole.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Underline links");
                let current = self.config.terminal.link_underline;
                egui::ComboBox::from_id_salt("link_underline")
                    .selected_text(current.label())
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for mode in terminale_config::LinkUnderline::all() {
                            if ui
                                .selectable_value(
                                    &mut self.config.terminal.link_underline,
                                    mode,
                                    mode.label(),
                                )
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Terminal, "Underline links");
            sublabel(
                ui,
                "When detected URLs are underlined. \"On hover\" (default) underlines only the link under the pointer, avoiding a stray accent line under startup banners.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Link hover tooltip");
                let on = self.config.terminal.link_hover_tooltip;
                if toggle_switch(ui, on).clicked() {
                    self.config.terminal.link_hover_tooltip = !on;
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
                Section::Terminal,
                "Link hover tooltip",
            );
            sublabel(
                ui,
                "Show a small tooltip with the resolved link target when hovering over a hyperlink. For OSC 8 links the tooltip shows the destination URL even when it differs from the visible text.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Link hover delay");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.terminal.link_hover_delay_ms, 0..=2000)
                        .suffix(" ms")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.terminal.link_hover_delay_ms = 0;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Terminal, "Link hover delay");
            sublabel(
                ui,
                "How long (ms) the pointer must dwell over a link before the hover tooltip appears. 0 = instant (default).",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Resize panes live while dragging");
                let on = self.config.terminal.live_pane_resize;
                if toggle_switch(ui, on).clicked() {
                    self.config.terminal.live_pane_resize = !on;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Terminal,
                "Resize panes live while dragging",
            );
            sublabel(
                ui,
                "On: PTYs resize on every cursor move during a divider drag (snappy local shell). \
                 Off: PTYs only learn the new size on release — useful for slow shells or SSH \
                 where every resize triggers a full repaint.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Keyboard pane resize step");
                let step = &mut self.config.terminal.pane_resize_step_cells;
                if ui
                    .add(egui::Slider::new(step, 1..=20).suffix(" cells"))
                    .changed()
                {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    *step = 2;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Terminal,
                "Keyboard pane resize step",
            );
            sublabel(
                ui,
                "How many cells the keyboard pane-resize actions \
                 (resize_pane_left/right/up/down) nudge the divider per press.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Prompt marks in gutter");
                let on = self.config.terminal.show_prompt_marks;
                if toggle_switch(ui, on).clicked() {
                    self.config.terminal.show_prompt_marks = !on;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Terminal,
                "Prompt marks in gutter",
            );
            sublabel(
                ui,
                "Draw a coloured dot in the left margin at each shell prompt (green = success, red = error). Requires OSC 133 shell integration.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "OS notifications");
                let on = self.config.terminal.os_notifications;
                if toggle_switch(ui, on).clicked() {
                    self.config.terminal.os_notifications = !on;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Terminal, "OS notifications");
            sublabel(
                ui,
                "Show an OS desktop notification when a program sends OSC 9 or OSC 777. Notifications are suppressed while the window is focused.",
            );
        });

        ui.add_space(6.0);

        card(ui, |ui| {
            // Presets for the editor launched on Ctrl+click of a
            // `file:line:col` reference. Empty command = OS default open.
            const PRESETS: [(&str, &str); 5] = [
                ("System default", ""),
                ("VS Code", "code -g {file}:{line}:{column}"),
                ("Sublime Text", "subl {file}:{line}:{column}"),
                ("Vim", "vim +{line} {file}"),
                ("Neovim", "nvim +{line} {file}"),
            ];
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Open file links with");
                let current = PRESETS
                    .iter()
                    .find(|(_, tpl)| *tpl == self.config.editor.command)
                    .map_or("Custom", |(label, _)| *label);
                egui::ComboBox::from_id_salt("editor_preset")
                    .selected_text(current)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        for (label, tpl) in PRESETS {
                            if ui
                                .selectable_value(
                                    &mut self.config.editor.command,
                                    tpl.to_string(),
                                    label,
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
                Section::Terminal,
                "Open file links with",
            );
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Command");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.config.editor.command)
                        .desired_width(360.0)
                        .hint_text("empty = OS default · tokens: {file} {line} {column}"),
                );
                if r.changed() {
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Terminal, "Command");
            sublabel(
                ui,
                "Ctrl+click a file:line:col reference (e.g. a compiler error) to jump there.",
            );
        });

        ui.add_space(6.0);

        // ── Exit behavior ─────────────────────────────────────────────────────
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Exit behavior");
                let current = self.config.terminal.exit_behavior;
                egui::ComboBox::from_id_salt("exit_behavior")
                    .selected_text(current.label())
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for mode in terminale_config::ExitBehavior::all() {
                            if ui
                                .selectable_value(
                                    &mut self.config.terminal.exit_behavior,
                                    mode,
                                    mode.label(),
                                )
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Terminal, "Exit behavior");
            sublabel(
                ui,
                "What happens when the program in a pane exits: \
                 \"Close\" (default) removes the pane immediately, \
                 \"Hold\" keeps it open with a status line, \
                 \"Close on clean exit\" auto-closes only on exit code 0.",
            );
        });

        ui.add_space(6.0);

        // ── Hyperlink rules ───────────────────────────────────────────────────
        card(ui, |ui| {
            field_label(ui, "Hyperlink rules");
            sublabel(
                ui,
                "Additional regex patterns matched against each visible row to create clickable links. \
                 When this list is non-empty, patterns are applied on top of the built-in URL scanner \
                 (http/https/ftp/file/mailto). An empty list uses built-in detection only.",
            );
            ui.add_space(4.0);

            // Editable list of regex rows.
            let mut to_remove: Option<usize> = None;
            {
                // Borrow the list mutably only for the loop; NLL ensures the
                // borrow ends before the post-loop mutations below.
                let rules = &mut self.config.terminal.hyperlink_rules;
                for (idx, rule) in rules.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        if ui
                            .small_button("✕")
                            .on_hover_text("Remove this rule")
                            .clicked()
                        {
                            to_remove = Some(idx);
                            dirty = true;
                        }
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut rule.regex)
                                .desired_width(260.0)
                                .hint_text("regex pattern"),
                        );
                        if r.changed() {
                            dirty = true;
                        }
                        let r2 = ui.add(
                            egui::TextEdit::singleline(&mut rule.label)
                                .desired_width(140.0)
                                .hint_text("label (optional)"),
                        );
                        if r2.changed() {
                            dirty = true;
                        }
                    });
                }
            } // rules borrow ends here
            if let Some(i) = to_remove {
                self.config.terminal.hyperlink_rules.remove(i);
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("+ Add rule").clicked() {
                    self.config
                        .terminal
                        .hyperlink_rules
                        .push(terminale_config::HyperlinkRule::default());
                    dirty = true;
                }
                if ui.button("Load defaults").clicked() {
                    self.config.terminal.hyperlink_rules =
                        terminale_config::default_hyperlink_rules();
                    dirty = true;
                }
                if ui.button("Clear all").clicked() {
                    self.config.terminal.hyperlink_rules.clear();
                    dirty = true;
                }
            });
        });

        ui.add_space(6.0);

        ui.add_space(6.0);

        // ── Keyboard encoding ─────────────────────────────────────────────────
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Keyboard encoding");
                let current = self.config.terminal.keyboard_encoding;
                egui::ComboBox::from_id_salt("keyboard_encoding")
                    .selected_text(current.label())
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for mode in terminale_config::KeyboardEncoding::all() {
                            if ui
                                .selectable_value(
                                    &mut self.config.terminal.keyboard_encoding,
                                    mode,
                                    mode.label(),
                                )
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Terminal, "Keyboard encoding");
            sublabel(
                ui,
                "\"Auto (honour app mode)\" (default): arrow keys and Home/End use \
                 application-mode sequences (SS3) when a program enables DECCKM — \
                 required for vim, less, htop, mc. \
                 \"Always CSI\": always emit CSI sequences regardless of DECCKM — \
                 a compatibility fallback for sessions that set DECCKM accidentally.",
            );
        });

        ui.add_space(6.0);

        // ── Inline image protocols ────────────────────────────────────────────
        card(ui, |ui| {
            field_label(ui, "Inline image protocols");
            sublabel(
                ui,
                "Which escape-sequence protocols are accepted for inline images. \
                 Disabling a protocol silently drops images from that source; other \
                 output is unaffected.",
            );
            ui.add_space(4.0);
            let hr = ui.horizontal(|ui| {
                let on = self.config.terminal.image_protocols.osc1337;
                if toggle_switch(ui, on).clicked() {
                    self.config.terminal.image_protocols.osc1337 = !on;
                    dirty = true;
                }
                ui.label("OSC 1337 inline images");
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Terminal,
                "Inline image protocols",
            );
            let hr = ui.horizontal(|ui| {
                let on = self.config.terminal.image_protocols.sixel;
                if toggle_switch(ui, on).clicked() {
                    self.config.terminal.image_protocols.sixel = !on;
                    dirty = true;
                }
                ui.label("Sixel (DCS)");
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Terminal,
                "Inline image protocols",
            );
            let hr = ui.horizontal(|ui| {
                let on = self.config.terminal.image_protocols.apc;
                if toggle_switch(ui, on).clicked() {
                    self.config.terminal.image_protocols.apc = !on;
                    dirty = true;
                }
                ui.label("APC graphics (ESC _G)");
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Terminal,
                "Inline image protocols",
            );
        });

        ui.add_space(6.0);

        // ── Clipboard read policy ────────────────────────────────────────────
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Clipboard read");
                let current = self.config.terminal.clipboard_read;
                egui::ComboBox::from_id_salt("clipboard_read_policy")
                    .selected_text(current.label())
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for policy in terminale_config::ClipboardReadPolicy::all() {
                            if ui
                                .selectable_value(
                                    &mut self.config.terminal.clipboard_read,
                                    policy,
                                    policy.label(),
                                )
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Terminal, "Clipboard read");
            sublabel(
                ui,
                "Permission for OSC 52 clipboard READ queries (the `?` payload). \
                 \"Deny\" (default) ignores the request — safe for all sessions, including remote shells. \
                 \"Allow\" replies with the clipboard contents encoded as base64; only enable this \
                 when every pane runs a program you trust completely.",
            );
        });

        ui.add_space(6.0);

        // ── Shell integration: command blocks ─────────────────────────────────
        card(ui, |ui| {
            field_label(ui, "Shell integration");
            sublabel(
                ui,
                "Capture each shell command as a discrete block using OSC 133 marks. \
                 Requires the shell to emit A/B/C/D sequences (fish, bash with bash-preexec, \
                 zsh with appropriate hooks). Blocks are the foundation for block-copy, re-run, \
                 and AI fix-on-fail.",
            );
            ui.add_space(4.0);

            let hr = ui.horizontal(|ui| {
                field_label(ui, "Capture command blocks");
                let on = self.config.terminal.command_blocks;
                if toggle_switch(ui, on).clicked() {
                    self.config.terminal.command_blocks = !on;
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
                Section::Terminal,
                "Capture command blocks",
            );

            let hr = ui.horizontal(|ui| {
                field_label(ui, "Max command blocks");
                let r = ui.add(
                    egui::Slider::new(&mut self.config.terminal.max_command_blocks, 1..=100_000)
                        .logarithmic(true)
                        .suffix(" blocks")
                        .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.terminal.max_command_blocks = 1000;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Terminal,
                "Max command blocks",
            );
            sublabel(
                ui,
                "Maximum blocks kept in memory per terminal. \
                 Oldest blocks are evicted when the cap is exceeded.",
            );

            let hr = ui.horizontal(|ui| {
                field_label(ui, "Edit command clears line");
                let on = self.config.terminal.edit_command_clears_line;
                if toggle_switch(ui, on).clicked() {
                    self.config.terminal.edit_command_clears_line = !on;
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
                Section::Terminal,
                "Edit command clears line",
            );
            sublabel(
                ui,
                "When enabled (default), the \"Edit Last Command\" action sends Ctrl+U \
                 (kill-line) before loading the command onto the prompt, clearing any \
                 partially-typed input. Disable if your shell binds Ctrl+U differently.",
            );

            ui.add_space(4.0);

            // ── Command-history picker ─────────────────────────────────────────
            let hr = ui.horizontal(|ui| {
                field_label(ui, "History picker scope");
                let current = self.config.terminal.command_history_scope;
                egui::ComboBox::from_id_salt("command_history_scope")
                    .selected_text(current.label())
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for scope in terminale_config::CommandHistoryScope::all() {
                            if ui
                                .selectable_value(
                                    &mut self.config.terminal.command_history_scope,
                                    scope,
                                    scope.label(),
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
                Section::Terminal,
                "History picker scope",
            );

            let hr = ui.horizontal(|ui| {
                field_label(ui, "History picker max entries");
                let r = ui.add(
                    egui::Slider::new(
                        &mut self.config.terminal.command_history_max_entries,
                        1..=10_000,
                    )
                    .logarithmic(true)
                    .suffix(" entries")
                    .text(""),
                );
                if r.changed() {
                    dirty = true;
                }
                if ui.small_button("Reset").clicked() {
                    self.config.terminal.command_history_max_entries = 500;
                    dirty = true;
                }
            });
            self.highlight_row(
                ui,
                hr.response.rect,
                Section::Terminal,
                "History picker max entries",
            );
            sublabel(
                ui,
                "Which panes the \"Command History\" picker gathers entries from, and how many \
                 deduplicated entries (newest first) it shows at most.",
            );
        });

        ui.add_space(6.0);

        // ── Broadcast input scope ─────────────────────────────────────────────
        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Broadcast input scope");
                let current = self.config.terminal.broadcast_scope;
                egui::ComboBox::from_id_salt("broadcast_scope")
                    .selected_text(current.label())
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for mode in terminale_config::BroadcastScope::all() {
                            if ui
                                .selectable_value(
                                    &mut self.config.terminal.broadcast_scope,
                                    mode,
                                    mode.label(),
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
                Section::Terminal,
                "Broadcast input scope",
            );
            sublabel(
                ui,
                "When broadcast-input mode is active (toggle via the command palette or a \
                 keyboard shortcut), keystrokes are mirrored to panes in this scope: \
                 \"All panes in tab\" (default) or \"All panes in window\". \
                 Receiving panes show a distinct amber border.",
            );
        });

        ui.add_space(6.0);

        // ── Scrollback export ─────────────────────────────────────────────────
        card(ui, |ui| {
            field_label(ui, "Scrollback export");
            sublabel(
                ui,
                "Settings for the \"Export Scrollback\" action (command palette or shortcut). \
                 The full buffer (history + visible screen) is written to a plain-text file.",
            );
            ui.add_space(4.0);

            let hr = ui.horizontal(|ui| {
                field_label(ui, "Export format");
                let current = self.config.terminal.scrollback_export_format;
                egui::ComboBox::from_id_salt("scrollback_export_format")
                    .selected_text(current.label())
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for fmt in terminale_config::ScrollbackExportFormat::all() {
                            if ui
                                .selectable_value(
                                    &mut self.config.terminal.scrollback_export_format,
                                    fmt,
                                    fmt.label(),
                                )
                                .clicked()
                            {
                                dirty = true;
                            }
                        }
                    });
            });
            self.highlight_row(ui, hr.response.rect, Section::Terminal, "Export format");

            let hr = ui.horizontal(|ui| {
                field_label(ui, "Export directory");
                let dir_str = self
                    .config
                    .terminal
                    .scrollback_export_dir
                    .as_deref()
                    .and_then(|p| p.to_str())
                    .unwrap_or("");
                let mut dir_edit = dir_str.to_string();
                let r = ui.add(
                    egui::TextEdit::singleline(&mut dir_edit)
                        .desired_width(280.0)
                        .hint_text("empty = open save dialog"),
                );
                if r.changed() {
                    if dir_edit.trim().is_empty() {
                        self.config.terminal.scrollback_export_dir = None;
                    } else {
                        self.config.terminal.scrollback_export_dir =
                            Some(std::path::PathBuf::from(dir_edit.trim()));
                    }
                    dirty = true;
                }
                if ui.small_button("Browse\u{2026}").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_parent(&*self.window)
                        .set_title("Choose export directory")
                        .pick_folder()
                    {
                        self.config.terminal.scrollback_export_dir = Some(path);
                        dirty = true;
                    }
                }
                if ui.small_button("Clear").clicked() {
                    self.config.terminal.scrollback_export_dir = None;
                    dirty = true;
                }
            });
            self.highlight_row(ui, hr.response.rect, Section::Terminal, "Export directory");
            sublabel(
                ui,
                "Directory to write exported scrollback files. Leave empty to open a save-file \
                 dialog each time. Files are named \"terminale-scrollback-YYYYMMDD-HHMMSS.txt\".",
            );
        });

        ui.add_space(6.0);

        // ── Paste safety ──────────────────────────────────────────────────────
        card(ui, |ui| {
            field_label(ui, "Paste safety");
            sublabel(
                ui,
                "Protect against clipboard-injection ('paste-jacking'): a hidden newline in \
                 pasted text can execute a command immediately when the focused program has \
                 not enabled bracketed paste. These controls let you review multi-line \
                 pastes before they reach the shell.",
            );
            ui.add_space(6.0);

            // Toggle: confirm when unbracketed (safety default = on)
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Confirm when unbracketed");
                let on = self.config.terminal.paste_confirm_when_unbracketed;
                if toggle_switch(ui, on).clicked() {
                    self.config.terminal.paste_confirm_when_unbracketed = !on;
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
                Section::Terminal,
                "Confirm when unbracketed",
            );
            sublabel(
                ui,
                "Ask for confirmation before pasting multi-line text when the focused \
                 program has NOT enabled bracketed paste (default: on). This is the \
                 primary clipboard-injection defence.",
            );

            ui.add_space(4.0);

            // Toggle: confirm all multi-line pastes
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Confirm multi-line paste");
                let on = self.config.terminal.paste_confirm_multiline;
                if toggle_switch(ui, on).clicked() {
                    self.config.terminal.paste_confirm_multiline = !on;
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
                Section::Terminal,
                "Confirm multi-line paste",
            );
            sublabel(
                ui,
                "Ask for confirmation before pasting multi-line text regardless of \
                 whether bracketed paste is active (default: off). Enable this if you \
                 want a preview prompt for every multi-line paste without exception.",
            );

            ui.add_space(4.0);

            // Toggle: strip control chars
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Strip control characters");
                let on = self.config.terminal.paste_strip_control_chars;
                if toggle_switch(ui, on).clicked() {
                    self.config.terminal.paste_strip_control_chars = !on;
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
                Section::Terminal,
                "Strip control characters",
            );
            sublabel(
                ui,
                "Remove non-printable control bytes (ESC, NUL, BEL, ...) from pasted \
                 text before sending to the PTY. Keeps newline, tab, and carriage-return. \
                 Applied to both confirmed and direct pastes (default: off).",
            );
        });

        // ── Prompt navigation ─────────────────────────────────────────────────

        ui.add_space(6.0);

        card(ui, |ui| {
            let hr = ui.horizontal(|ui| {
                field_label(ui, "Highlight on jump");
                let on = self.config.terminal.highlight_on_jump;
                if toggle_switch(ui, on).clicked() {
                    self.config.terminal.highlight_on_jump = !on;
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
            self.highlight_row(ui, hr.response.rect, Section::Terminal, "Highlight on jump");
            sublabel(
                ui,
                "Briefly tint the target prompt row when jumping to a previous or next \
                 prompt / failed command (OSC 133 shell integration). The band fades \
                 over ~400 ms. Requires shell integration. Default: on.",
            );
        });

        // ── Live-apply new config values when dirty ───────────────────────────
        // Sync module-level atomics that don't go through the RunningState
        // config-mirror pattern (because they don't have a field on TermWindow).
        if dirty {
            crate::osc_handlers::update_exit_behavior(self.config.terminal.exit_behavior);
            crate::links::update_hyperlink_rules(&self.config.terminal.hyperlink_rules);
        }

        if dirty {
            self.dirty = true;
        }
    }
}
